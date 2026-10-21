use crate::diagnostics::hydrate_session_bus_env;
use anyhow::{anyhow, Context, Result};
use atspi::{
    proxy::{
        accessible::{AccessibleProxy, ObjectRefExt},
        proxy_ext::ProxyExt,
    },
    CoordType, ObjectRef, ObjectRefOwned, StateSet,
};
use futures_util::{stream, StreamExt};
use schemars::JsonSchema;
use serde::Serialize;
use std::{collections::VecDeque, future::Future, time::Duration};
use tokio::time::timeout;
use zbus::Connection;
use zbus::{
    fdo::DBusProxy,
    names::{BusName, UniqueName},
    zvariant::ObjectPath,
};

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AccessibleAppSummary {
    pub object_ref: String,
    pub name: Option<String>,
    pub pid: Option<u32>,
    pub role: String,
    pub child_count: i32,
    pub bounds: Option<Bounds>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AccessibilityNode {
    pub index: u32,
    pub parent_index: Option<u32>,
    pub depth: u32,
    pub object_ref: String,
    pub role: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub child_count: i32,
    pub bounds: Option<Bounds>,
    pub states: Vec<String>,
    pub actions: Vec<AccessibilityAction>,
    pub value: Option<AccessibilityValue>,
    pub text: Option<AccessibilityText>,
    pub supports_editable_text: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AccessibilityAction {
    pub index: i32,
    pub name: String,
    pub description: String,
    pub keybinding: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AccessibilityValue {
    pub current: f64,
    pub minimum: f64,
    pub maximum: f64,
    pub minimum_increment: f64,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AccessibilityText {
    pub character_count: i32,
    pub caret_offset: Option<i32>,
    pub content: Option<String>,
    pub truncated: bool,
    pub selections: Vec<AccessibilityTextSelection>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AccessibilityTextSelection {
    pub start_offset: i32,
    pub end_offset: i32,
}

#[derive(Debug, Clone)]
pub struct ActionInvocation {
    pub action_index: i32,
    pub action_name: Option<String>,
    pub ok: bool,
}

#[derive(Debug, Clone)]
pub enum ValueSetInvocation {
    Numeric { value: f64 },
    EditableText,
}

const MAX_TEXT_READBACK_CHARS: i32 = 4096;
const MAX_TEXT_SELECTIONS: i32 = 8;
const DEFAULT_SNAPSHOT_MAX_NODES: usize = 1_000;
const HARD_SNAPSHOT_MAX_NODES: usize = 2_000;
const DEFAULT_SNAPSHOT_MAX_DEPTH: u32 = 32;
const HARD_SNAPSHOT_MAX_DEPTH: u32 = 64;
const CHILD_READ_CONCURRENCY: usize = 16;
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_DISCOVERY_ROOTS: usize = 256;
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(3);
const PROVIDER_TIMEOUT: Duration = Duration::from_millis(300);
const MAX_METADATA_BYTES: usize = 4096;
const MAX_ACTIONS: usize = 64;
const MAX_SNAPSHOT_METADATA_BYTES: usize = 4 * 1024 * 1024;
const ROOT_MATCH_CHILD_LIMIT: usize = 8;
const MAX_DISCOVERY_CHILD_READS: usize = MAX_DISCOVERY_ROOTS * ROOT_MATCH_CHILD_LIMIT;

fn snapshot_child_read_budgets(max_nodes: usize) -> (usize, usize, usize) {
    (MAX_DISCOVERY_ROOTS, MAX_DISCOVERY_CHILD_READS, max_nodes)
}

pub(crate) fn snapshot_limits(
    requested_max_nodes: Option<usize>,
    requested_max_depth: Option<u32>,
) -> (usize, u32) {
    (
        requested_max_nodes
            .unwrap_or(DEFAULT_SNAPSHOT_MAX_NODES)
            .clamp(1, HARD_SNAPSHOT_MAX_NODES),
        requested_max_depth
            .unwrap_or(DEFAULT_SNAPSHOT_MAX_DEPTH)
            .min(HARD_SNAPSHOT_MAX_DEPTH),
    )
}

struct BoundedTraversal<T> {
    queue: VecDeque<T>,
    attempted: usize,
    max_items: usize,
    /// Set once an enqueue had to drop offered items for lack of capacity.
    dropped: bool,
}

impl<T> BoundedTraversal<T> {
    fn new(max_items: usize) -> Self {
        Self {
            queue: VecDeque::new(),
            attempted: 0,
            max_items,
            dropped: false,
        }
    }

    fn enqueue(&mut self, items: impl IntoIterator<Item = T>) {
        let capacity = self.remaining_capacity();
        let mut items = items.into_iter();
        self.queue.extend(items.by_ref().take(capacity));
        if items.next().is_some() {
            self.dropped = true;
        }
    }

    fn pop(&mut self) -> Option<T> {
        if self.attempted >= self.max_items {
            return None;
        }
        let item = self.queue.pop_front()?;
        self.attempted += 1;
        Some(item)
    }

    fn remaining_capacity(&self) -> usize {
        self.max_items
            .saturating_sub(self.attempted.saturating_add(self.queue.len()))
    }

    /// True when the budget stopped work that was actually offered. `enqueue`
    /// is the only gate: it never exceeds `remaining_capacity`, so
    /// `attempted + queue.len() <= max_items` always holds and the queue can
    /// never be non-empty at the attempt cap. Dropped items are the whole story.
    fn truncated(&self) -> bool {
        self.dropped
    }
}

fn bounded_child_count(reported: i32, limit: usize) -> usize {
    usize::try_from(reported).unwrap_or_default().min(limit)
}

struct IndexedReadBatch<T> {
    items: Vec<T>,
    attempted: usize,
    /// True when fewer children were attempted than the parent reported, because
    /// the caller's limit or the shared read budget ran out. Failed reads do not
    /// count; they were attempted.
    incomplete: bool,
}

impl<T> IndexedReadBatch<T> {
    fn all_failed(&self) -> bool {
        self.attempted > 0 && self.items.is_empty()
    }
}

async fn fetch_indexed_up_to<T, E, F, Fut>(
    reported: i32,
    limit: usize,
    remaining_attempts: &mut usize,
    fetch: F,
) -> IndexedReadBatch<T>
where
    F: Fn(i32) -> Fut,
    Fut: Future<Output = std::result::Result<T, E>>,
{
    let attempt_count = bounded_child_count(reported, limit).min(*remaining_attempts);
    *remaining_attempts = (*remaining_attempts).saturating_sub(attempt_count);
    let end_index = i32::try_from(attempt_count).unwrap_or(i32::MAX);

    let items = stream::iter(0..end_index)
        .map(fetch)
        .buffered(CHILD_READ_CONCURRENCY)
        .filter_map(|result| async move { result.ok() })
        .collect()
        .await;

    IndexedReadBatch {
        items,
        attempted: attempt_count,
        incomplete: attempt_count < usize::try_from(reported).unwrap_or_default(),
    }
}

async fn children_up_to(
    proxy: &AccessibleProxy<'_>,
    limit: usize,
    remaining_attempts: &mut usize,
) -> zbus::Result<IndexedReadBatch<ObjectRefOwned>> {
    if limit == 0 || *remaining_attempts == 0 {
        // Budget already spent: stay I/O-free. `incomplete` is false here because
        // nothing was asked of the parent; callers that already hold the node's
        // child_count (snapshot_tree_inner) derive truncation from that instead.
        return Ok(IndexedReadBatch {
            items: Vec::new(),
            attempted: 0,
            incomplete: false,
        });
    }

    let child_count = proxy.child_count().await?;
    let mut batch = fetch_indexed_up_to(child_count, limit, remaining_attempts, |index| {
        proxy.get_child_at_index(index)
    })
    .await;
    let original_len = batch.items.len();
    batch.items.retain(valid_object_ref_metadata);
    batch.incomplete |= batch.items.len() != original_len;
    Ok(batch)
}

pub async fn list_accessible_apps(limit: usize) -> Result<Vec<AccessibleAppSummary>> {
    list_accessible_apps_with_connection(limit, DISCOVERY_TIMEOUT, connect()).await
}

// Keep completed summaries outside the timed future so cancellation retains
// useful discovery results, including when the registry or a provider wedges.
async fn list_accessible_apps_with_connection(
    limit: usize,
    deadline: Duration,
    connection: impl Future<Output = Result<Connection>>,
) -> Result<Vec<AccessibleAppSummary>> {
    let limit = limit.min(MAX_DISCOVERY_ROOTS);
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut apps = Vec::new();
    let result = timeout(deadline, async {
        let conn = connection.await?;
        let mut remaining_child_reads = limit;
        let roots = registry_children(&conn, limit, &mut remaining_child_reads).await?;
        let dbus = DBusProxy::new(&conn).await.ok();
        for object_ref in roots.into_iter().take(limit) {
            let summary = timeout(PROVIDER_TIMEOUT, async {
                let proxy = open_accessible(&conn, &object_ref).await.ok()?;
                Some(read_app_summary(&proxy, &object_ref, dbus.as_ref()).await)
            })
            .await;
            if let Ok(Some(summary)) = summary {
                apps.push(summary);
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .await;
    match result {
        Ok(result) => result?,
        Err(_) if apps.is_empty() => {
            return Err(anyhow!("AT-SPI discovery exceeded its deadline"));
        }
        Err(_) => {} // Best-effort: return summaries completed before the deadline.
    }
    Ok(apps)
}

#[derive(Debug, Clone)]
pub(crate) struct AccessibilitySnapshot {
    pub nodes: Vec<AccessibilityNode>,
    /// True when registry roots were filtered to an app name and/or pid.
    pub scoped: bool,
    /// The pid every root belongs to, when roots were matched by pid. None
    /// when they were matched by app name alone or not filtered at all: those
    /// roots can belong to any app, so no single pid describes the tree.
    pub root_pid: Option<u32>,
    /// True when node, depth, child-read, or retained-metadata budgets stopped
    /// traversal with unread elements left. Failed element reads do not count.
    pub truncated: bool,
}

pub async fn snapshot_tree(
    app_name_or_bundle_identifier: Option<&str>,
    target_pid: Option<u32>,
    max_nodes: usize,
    max_depth: u32,
) -> Result<Vec<AccessibilityNode>> {
    Ok(snapshot_accessibility_tree(
        app_name_or_bundle_identifier,
        target_pid,
        max_nodes,
        max_depth,
    )
    .await?
    .nodes)
}

pub(crate) async fn snapshot_accessibility_tree(
    app_name_or_bundle_identifier: Option<&str>,
    target_pid: Option<u32>,
    max_nodes: usize,
    max_depth: u32,
) -> Result<AccessibilitySnapshot> {
    let (max_nodes, max_depth) = snapshot_limits(Some(max_nodes), Some(max_depth));
    timeout(
        SNAPSHOT_TIMEOUT,
        snapshot_tree_inner(
            app_name_or_bundle_identifier,
            target_pid,
            max_nodes,
            max_depth,
        ),
    )
    .await
    .context("AT-SPI snapshot exceeded its 10-second deadline")?
}

async fn snapshot_tree_inner(
    app_name_or_bundle_identifier: Option<&str>,
    target_pid: Option<u32>,
    max_nodes: usize,
    max_depth: u32,
) -> Result<AccessibilitySnapshot> {
    let conn = connect().await?;
    snapshot_tree_on_connection(
        &conn,
        app_name_or_bundle_identifier,
        target_pid,
        max_nodes,
        max_depth,
        MAX_SNAPSHOT_METADATA_BYTES,
    )
    .await
}

async fn snapshot_tree_on_connection(
    conn: &Connection,
    app_name_or_bundle_identifier: Option<&str>,
    target_pid: Option<u32>,
    max_nodes: usize,
    max_depth: u32,
    metadata_budget: usize,
) -> Result<AccessibilitySnapshot> {
    // App discovery is bounded independently so a tiny requested tree still
    // finds a target registered after the first accessibility root.
    let (mut remaining_registry_reads, mut remaining_filter_reads, mut remaining_traversal_reads) =
        snapshot_child_read_budgets(max_nodes);
    let roots = registry_children(conn, MAX_DISCOVERY_ROOTS, &mut remaining_registry_reads).await?;
    let selected_roots = select_roots(
        conn,
        roots,
        app_name_or_bundle_identifier,
        target_pid,
        &mut remaining_filter_reads,
    )
    .await;
    let scoped = selected_roots.scoped;
    let root_pid = selected_roots.pid;
    let mut nodes = Vec::new();
    let mut remaining_metadata = metadata_budget;
    let mut truncated = false;
    let mut traversal = BoundedTraversal::new(max_nodes);

    traversal.enqueue(
        selected_roots
            .roots
            .into_iter()
            .map(|object_ref| (object_ref, 0_u32, None)),
    );

    while let Some((object_ref, depth, parent_index)) = traversal.pop() {
        let Ok(proxy) = open_accessible(conn, &object_ref).await else {
            continue;
        };
        let index = nodes.len() as u32;
        let node = read_node(&proxy, &object_ref, index, parent_index, depth).await;
        let Some(remaining) = remaining_metadata.checked_sub(node_metadata_bytes(&node)) else {
            truncated = true;
            break;
        };
        remaining_metadata = remaining;
        let remaining = traversal.remaining_capacity();
        let child_refs = if depth < max_depth && remaining > 0 {
            match children_up_to(&proxy, remaining, &mut remaining_traversal_reads).await {
                Ok(batch) => {
                    // A read-budget cut inside the fetch reports `incomplete`; an
                    // already-exhausted budget returns the I/O-free empty batch, so
                    // fall back to the child_count read_node already fetched.
                    truncated |= batch.incomplete || (batch.attempted == 0 && node.child_count > 0);
                    batch.items
                }
                Err(_) => Vec::new(),
            }
        } else {
            // Depth or node cap reached: any child this node reports is unread.
            truncated |= node.child_count > 0;
            Vec::new()
        };

        nodes.push(node);

        traversal.enqueue(
            child_refs
                .into_iter()
                .map(|child| (child, depth + 1, Some(index))),
        );
    }
    truncated |= traversal.truncated();

    Ok(AccessibilitySnapshot {
        nodes,
        scoped,
        root_pid,
        truncated,
    })
}

/// Compact description of the AT-SPI element that currently holds keyboard
/// focus, used as post-input feedback for type_text/press_key.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FocusedElementSummary {
    pub role: String,
    pub name: Option<String>,
    pub editable: bool,
    pub states: Vec<String>,
    /// AT-SPI `Role::Terminal`, read from the role enum rather than the
    /// localized role name. Internal routing only; not part of the output.
    #[serde(skip)]
    #[schemars(skip)]
    pub is_terminal: bool,
}

const FOCUS_PROBE_MAX_NODES: usize = 400;
const FOCUS_PROBE_MAX_DEPTH: u32 = 16;

/// Find the element with the `focused` state inside the target app (by pid) or
/// across all apps. Best-effort and bounded: returns Ok(None) when no focused
/// element is reachable through AT-SPI (common for apps without accessibility
/// support, e.g. Electron without --force-renderer-accessibility).
/// Outcome of the bounded focused-element search.
#[derive(Debug, Clone)]
pub(crate) enum FocusProbe {
    Found(FocusedElementSummary),
    /// The search covered the target's whole tree and nothing holds focus.
    NoneFocused,
    /// A node, depth, or read limit stopped the search before it finished, so
    /// the focused element may exist past the limit.
    Incomplete,
    /// A pid was given but no AT-SPI app belongs to it (xterm, or Electron
    /// without --force-renderer-accessibility), so focus cannot be read.
    NoAccessibleApp,
}

/// Focused element in the app owning `target_pid`, or across all apps when no
/// pid is given. With a pid, only that app's tree is searched: falling back to
/// every other app would report some unrelated widget as the focus.
pub(crate) async fn probe_focused_element(target_pid: Option<u32>) -> Result<FocusProbe> {
    focused_element_probe(target_pid, target_pid.is_some()).await
}

/// The focused element in the app that owns `target_pid`, for callers that
/// only need an answer when one exists. Every non-`Found` probe result maps to
/// `None`; see [`probe_focused_element`] for the distinctions.
pub(crate) async fn focused_element_summary_in_app(
    target_pid: u32,
) -> Result<Option<FocusedElementSummary>> {
    Ok(match focused_element_probe(Some(target_pid), true).await? {
        FocusProbe::Found(summary) => Some(summary),
        _ => None,
    })
}

async fn focused_element_probe(
    target_pid: Option<u32>,
    require_scoped: bool,
) -> Result<FocusProbe> {
    let conn = connect().await?;
    let mut remaining_registry_reads = MAX_DISCOVERY_ROOTS;
    let roots =
        registry_children(&conn, MAX_DISCOVERY_ROOTS, &mut remaining_registry_reads).await?;
    let mut remaining_filter_reads = MAX_DISCOVERY_CHILD_READS;
    let selected_roots =
        select_roots(&conn, roots, None, target_pid, &mut remaining_filter_reads).await;
    if require_scoped && !selected_roots.scoped {
        return Ok(FocusProbe::NoAccessibleApp);
    }
    let mut traversal = BoundedTraversal::new(FOCUS_PROBE_MAX_NODES);
    let mut remaining_traversal_reads = FOCUS_PROBE_MAX_NODES;
    let mut incomplete = false;

    traversal.enqueue(
        selected_roots
            .roots
            .into_iter()
            .map(|object_ref| (object_ref, 0_u32)),
    );

    while let Some((object_ref, depth)) = traversal.pop() {
        let Ok(proxy) = open_accessible(&conn, &object_ref).await else {
            continue;
        };
        let Ok(state) = proxy.get_state().await else {
            continue;
        };
        if state.contains(atspi::State::Focused) {
            let proxies = proxy.proxies().await.ok();
            let is_terminal = matches!(proxy.get_role().await, Ok(atspi::Role::Terminal));
            return Ok(FocusProbe::Found(FocusedElementSummary {
                role: role_name(&proxy).await,
                name: optional_string(proxy.name().await.ok()),
                editable: supports_editable_text(proxies.as_ref()).await,
                states: state_labels(state),
                is_terminal,
            }));
        }
        let remaining = traversal.remaining_capacity();
        if depth < FOCUS_PROBE_MAX_DEPTH && remaining > 0 {
            if let Ok(batch) =
                children_up_to(&proxy, remaining, &mut remaining_traversal_reads).await
            {
                incomplete |= batch.incomplete;
                // An exhausted shared read budget takes children_up_to's
                // I/O-free path (attempted 0, incomplete false), so the node's
                // own child count has to say whether children went unread.
                // With budget left, attempted 0 just means a leaf: no read.
                if batch.attempted == 0 && remaining_traversal_reads == 0 && !incomplete {
                    incomplete = proxy.child_count().await.is_ok_and(|count| count > 0);
                }
                traversal.enqueue(batch.items.into_iter().map(|child| (child, depth + 1)));
            }
        } else if !incomplete {
            // Depth or node cap: any child this node reports goes unread. One
            // child-count read settles it, and only until the first cut is seen.
            incomplete = proxy.child_count().await.is_ok_and(|count| count > 0);
        }
    }
    if incomplete || traversal.truncated() {
        return Ok(FocusProbe::Incomplete);
    }

    Ok(FocusProbe::NoneFocused)
}

pub async fn perform_action(
    object_ref_id: &str,
    requested_action: Option<&str>,
) -> Result<ActionInvocation> {
    perform_action_inner(object_ref_id, requested_action, false).await
}

pub(crate) async fn perform_named_action(
    object_ref_id: &str,
    name: &str,
) -> Result<ActionInvocation> {
    perform_action_inner(object_ref_id, Some(name), true).await
}

async fn perform_action_inner(
    object_ref_id: &str,
    requested_action: Option<&str>,
    name_only: bool,
) -> Result<ActionInvocation> {
    let conn = connect_for_mutation().await?;
    perform_action_on_connection(&conn, object_ref_id, requested_action, name_only).await
}

async fn perform_action_on_connection(
    conn: &Connection,
    object_ref_id: &str,
    requested_action: Option<&str>,
    name_only: bool,
) -> Result<ActionInvocation> {
    let object_ref = object_ref_from_id(object_ref_id)?;
    let (action, action_index, action_name) = timeout(DISCOVERY_TIMEOUT, async {
        let proxy = open_accessible(conn, &object_ref)
            .await
            .with_context(|| format!("failed to open AT-SPI object {object_ref_id}"))?;
        let action = proxy
            .proxies()
            .await?
            .action()
            .await
            .context("element does not expose the AT-SPI Action interface")?;
        let actions = read_actions(&action).await?;
        let action_index = if name_only {
            select_named_action_index(&actions, requested_action.unwrap_or_default())?
        } else {
            select_action_index(&actions, requested_action)?
        };
        let action_name = actions
            .get(action_index as usize)
            .map(|action| action.name.clone());
        Ok::<_, anyhow::Error>((action, action_index, action_name))
    })
    .await
    .context("AT-SPI action metadata exceeded its deadline")??;
    // Once sent, a mutation may finish after an observation deadline. Preserve
    // its completion/error semantics instead of releasing the input lock early.
    let ok = action
        .do_action(action_index)
        .await
        .with_context(|| format!("failed to invoke AT-SPI action {action_index}"))?;

    Ok(ActionInvocation {
        action_index,
        action_name,
        ok,
    })
}

pub async fn set_element_value(object_ref_id: &str, value: &str) -> Result<ValueSetInvocation> {
    let conn = connect_for_mutation().await?;
    set_element_value_on_connection(&conn, object_ref_id, value).await
}

async fn set_element_value_on_connection(
    conn: &Connection,
    object_ref_id: &str,
    value: &str,
) -> Result<ValueSetInvocation> {
    let object_ref = object_ref_from_id(object_ref_id)?;
    let proxies = timeout(DISCOVERY_TIMEOUT, async {
        let proxy = open_accessible(conn, &object_ref)
            .await
            .with_context(|| format!("failed to open AT-SPI object {object_ref_id}"))?;
        proxy.proxies().await.map_err(anyhow::Error::from)
    })
    .await
    .context("AT-SPI value metadata exceeded its deadline")??;

    if let Ok(numeric_value) = value.parse::<f64>() {
        if let Ok(value_proxy) = proxies.value().await {
            value_proxy
                .set_current_value(numeric_value)
                .await
                .with_context(|| {
                    format!("failed to set AT-SPI numeric value to {numeric_value}")
                })?;
            return Ok(ValueSetInvocation::Numeric {
                value: numeric_value,
            });
        }
    }

    if let Ok(editable_text) = proxies.editable_text().await {
        let ok = editable_text
            .set_text_contents(value)
            .await
            .context("failed to set AT-SPI editable text contents")?;
        if ok {
            return Ok(ValueSetInvocation::EditableText);
        }
        return Err(anyhow!("AT-SPI EditableText rejected the new contents"));
    }

    if value.parse::<f64>().is_err() && proxies.value().await.is_ok() {
        return Err(anyhow!(
            "element exposes the AT-SPI Value interface, but the requested value is not numeric"
        ));
    }

    Err(anyhow!(
        "element does not expose AT-SPI Value or EditableText interfaces"
    ))
}

async fn connect() -> Result<Connection> {
    hydrate_session_bus_env();
    crate::atspi_transport::connect()
        .await
        .context("failed to connect to AT-SPI bus")
}

async fn connect_for_mutation() -> Result<Connection> {
    hydrate_session_bus_env();
    crate::atspi_transport::connect_for_mutation()
        .await
        .context("failed to connect to AT-SPI bus")
}

/// Open an `AccessibleProxy` for an object on the a11y bus.
///
/// We deliberately avoid `AccessibilityConnection::object_as_accessible` (the
/// `P2P` trait). For apps that advertise a peer-to-peer bus address it routes
/// reads over that socket, but for apps that don't (notably GTK4 apps such as
/// Nautilus / Text Editor / baobab, which don't implement the legacy
/// `GetApplicationBusAddress`) it falls back to a proxy built with only a path
/// and *no destination*. On the shared a11y bus that proxy can't address the
/// app and every call fails with `ServiceUnknown`, which surfaces as an empty
/// tree (`role: "unknown"`, `child_count: 0`). `as_accessible_proxy` always
/// pins the destination to the object's bus name, so it works for every app
/// regardless of P2P support. See issue #31.
async fn open_accessible<'r>(
    conn: &Connection,
    object_ref: &'r ObjectRefOwned,
) -> Result<AccessibleProxy<'r>, atspi::AtspiError> {
    object_ref.as_accessible_proxy(conn).await
}

async fn registry_children(
    conn: &Connection,
    limit: usize,
    remaining_child_reads: &mut usize,
) -> Result<Vec<ObjectRefOwned>> {
    let root = AccessibleProxy::builder(conn)
        .destination("org.a11y.atspi.Registry")?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
        .context("failed to open AT-SPI registry root")?;
    let batch = children_up_to(&root, limit, remaining_child_reads)
        .await
        .context("failed to read AT-SPI registry children")?;
    if batch.all_failed() {
        return Err(anyhow!(
            "AT-SPI registry reported children, but every indexed child read failed"
        ));
    }
    Ok(batch.items)
}

struct SelectedRoots {
    roots: Vec<ObjectRefOwned>,
    scoped: bool,
    /// Set only when the roots were chosen because they belong to this pid.
    pid: Option<u32>,
}

/// Normalized app-name filter, or `None` when the caller passed nothing usable.
/// A `None` needle with no pid match means the snapshot covers the whole desktop.
fn app_name_needle(app_name_or_bundle_identifier: Option<&str>) -> Option<String> {
    app_name_or_bundle_identifier
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
}

async fn select_roots(
    conn: &Connection,
    roots: Vec<ObjectRefOwned>,
    app_name_or_bundle_identifier: Option<&str>,
    target_pid: Option<u32>,
    remaining_child_reads: &mut usize,
) -> SelectedRoots {
    let needle = app_name_needle(app_name_or_bundle_identifier);
    let dbus = DBusProxy::new(conn).await.ok();
    let mut remaining = roots;

    if let Some(target_pid) = target_pid {
        let mut pid_and_filter_matches = Vec::new();
        let mut pid_matches = Vec::new();
        let mut non_pid_matches = Vec::new();

        for object_ref in remaining {
            if object_ref_pid(dbus.as_ref(), &object_ref).await == Some(target_pid) {
                if let Some(needle) = needle.as_deref() {
                    if root_matches(conn, &object_ref, needle, remaining_child_reads).await {
                        pid_and_filter_matches.push(object_ref);
                    } else {
                        pid_matches.push(object_ref);
                    }
                } else {
                    pid_matches.push(object_ref);
                }
            } else {
                non_pid_matches.push(object_ref);
            }
        }

        if !pid_and_filter_matches.is_empty() {
            return SelectedRoots {
                roots: pid_and_filter_matches,
                scoped: true,
                pid: Some(target_pid),
            };
        }
        if !pid_matches.is_empty() {
            return SelectedRoots {
                roots: pid_matches,
                scoped: true,
                pid: Some(target_pid),
            };
        }

        remaining = non_pid_matches;
    }

    let Some(needle) = needle.as_deref() else {
        return SelectedRoots {
            roots: remaining,
            scoped: false,
            pid: None,
        };
    };

    let mut selected = Vec::new();
    for object_ref in remaining {
        if root_matches(conn, &object_ref, needle, remaining_child_reads).await {
            selected.push(object_ref);
        }
    }

    SelectedRoots {
        roots: selected,
        scoped: true,
        pid: None,
    }
}

async fn root_matches(
    conn: &Connection,
    object_ref: &ObjectRefOwned,
    needle: &str,
    remaining_child_reads: &mut usize,
) -> bool {
    let Ok(proxy) = open_accessible(conn, object_ref).await else {
        return object_ref_id(object_ref)
            .to_ascii_lowercase()
            .contains(needle);
    };

    if proxy_matches(&proxy, object_ref, needle).await {
        return true;
    }

    for child_ref in children_up_to(&proxy, ROOT_MATCH_CHILD_LIMIT, remaining_child_reads)
        .await
        .map(|batch| batch.items)
        .unwrap_or_default()
    {
        let Ok(child_proxy) = open_accessible(conn, &child_ref).await else {
            continue;
        };
        if proxy_matches(&child_proxy, &child_ref, needle).await {
            return true;
        }
    }

    false
}

async fn proxy_matches(
    proxy: &AccessibleProxy<'_>,
    object_ref: &ObjectRefOwned,
    needle: &str,
) -> bool {
    let name = optional_string(proxy.name().await.ok()).unwrap_or_default();
    let role = optional_string(proxy.get_role_name().await.ok()).unwrap_or_default();
    format!("{} {} {}", object_ref_id(object_ref), name, role)
        .to_ascii_lowercase()
        .contains(needle)
}

async fn read_app_summary(
    proxy: &AccessibleProxy<'_>,
    object_ref: &ObjectRefOwned,
    dbus: Option<&DBusProxy<'_>>,
) -> AccessibleAppSummary {
    AccessibleAppSummary {
        object_ref: object_ref_id(object_ref),
        name: optional_string(proxy.name().await.ok()),
        pid: object_ref_pid(dbus, object_ref).await,
        role: role_name(proxy).await,
        child_count: proxy.child_count().await.unwrap_or_default(),
        bounds: bounds(proxy).await,
    }
}

async fn read_node(
    proxy: &AccessibleProxy<'_>,
    object_ref: &ObjectRefOwned,
    index: u32,
    parent_index: Option<u32>,
    depth: u32,
) -> AccessibilityNode {
    let proxies = proxy.proxies().await.ok();

    AccessibilityNode {
        index,
        parent_index,
        depth,
        object_ref: object_ref_id(object_ref),
        role: role_name(proxy).await,
        name: optional_string(proxy.name().await.ok()),
        description: optional_string(proxy.description().await.ok()),
        child_count: proxy.child_count().await.unwrap_or_default(),
        bounds: bounds_from_proxies(proxies.as_ref(), proxy).await,
        states: states_from_proxy(proxy).await,
        actions: actions_from_proxies(proxies.as_ref()).await,
        value: value_from_proxies(proxies.as_ref()).await,
        text: text_from_proxies(proxies.as_ref()).await,
        supports_editable_text: supports_editable_text(proxies.as_ref()).await,
    }
}

// A node cap alone still permits thousands of individually valid large replies.
// Charge retained metadata before adding the node to the snapshot or its cache.
fn node_metadata_bytes(node: &AccessibilityNode) -> usize {
    let mut bytes = node.object_ref.len() + node.role.len();
    bytes += node.name.as_ref().map_or(0, String::len);
    bytes += node.description.as_ref().map_or(0, String::len);
    bytes += node.states.iter().map(String::len).sum::<usize>();
    bytes += node
        .actions
        .iter()
        .map(|action| action.name.len() + action.description.len() + action.keybinding.len())
        .sum::<usize>();
    bytes += node
        .value
        .as_ref()
        .and_then(|value| value.text.as_ref())
        .map_or(0, String::len);
    bytes += node
        .text
        .as_ref()
        .and_then(|text| text.content.as_ref())
        .map_or(0, String::len);
    bytes
}

async fn role_name(proxy: &AccessibleProxy<'_>) -> String {
    if let Ok(role) = proxy.get_role_name().await {
        if role.len() <= MAX_METADATA_BYTES && !role.trim().is_empty() {
            return role;
        }
    }
    proxy
        .get_role()
        .await
        .map(|role| format!("{role:?}"))
        .unwrap_or_else(|_| "unknown".to_string())
}

async fn bounds(proxy: &AccessibleProxy<'_>) -> Option<Bounds> {
    bounds_from_proxies(proxy.proxies().await.ok().as_ref(), proxy).await
}

/// Pid of the process that owns an AT-SPI object ref (`:bus/path`), from the
/// accessibility bus. `None` when the owner is gone or the bus cannot say.
pub(crate) async fn object_ref_owner_pid(object_ref_id: &str) -> Result<Option<u32>> {
    let object_ref = object_ref_from_id(object_ref_id)?;
    let conn = connect().await?;
    let dbus = DBusProxy::new(&conn).await.ok();
    Ok(object_ref_pid(dbus.as_ref(), &object_ref).await)
}

async fn object_ref_pid(dbus: Option<&DBusProxy<'_>>, object_ref: &ObjectRefOwned) -> Option<u32> {
    let dbus = dbus?;
    let bus_name = BusName::try_from(object_ref.name_as_str()?.to_string()).ok()?;
    dbus.get_connection_unix_process_id(bus_name).await.ok()
}

async fn bounds_from_proxies(
    proxies: Option<&atspi::proxy::proxy_ext::Proxies<'_>>,
    proxy: &AccessibleProxy<'_>,
) -> Option<Bounds> {
    let owned_proxies;
    let proxies = if let Some(proxies) = proxies {
        proxies
    } else {
        owned_proxies = proxy.proxies().await.ok()?;
        &owned_proxies
    };
    let component = proxies.component().await.ok()?;
    let (x, y, width, height) = component.get_extents(CoordType::Screen).await.ok()?;
    normalize_bounds(Bounds {
        x,
        y,
        width,
        height,
    })
}

fn normalize_bounds(bounds: Bounds) -> Option<Bounds> {
    if bounds.width <= 0 || bounds.height <= 0 {
        return None;
    }
    if bounds.x <= i32::MIN / 2 || bounds.y <= i32::MIN / 2 {
        return None;
    }
    Some(bounds)
}

async fn actions_from_proxies(
    proxies: Option<&atspi::proxy::proxy_ext::Proxies<'_>>,
) -> Vec<AccessibilityAction> {
    let Some(proxies) = proxies else {
        return Vec::new();
    };
    let Ok(action_proxy) = proxies.action().await else {
        return Vec::new();
    };

    read_actions(&action_proxy)
        .await
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(index, action)| AccessibilityAction {
            index: index as i32,
            name: action.name,
            description: action.description,
            keybinding: action.keybinding,
        })
        .collect()
}

// Use the same complete collection for observation and action revalidation.
// Reject excess metadata instead of truncating names or changing action indices.
async fn read_actions(proxy: &atspi::proxy::action::ActionProxy<'_>) -> Result<Vec<atspi::Action>> {
    validate_actions(proxy.get_actions().await?)
}

fn validate_actions(actions: Vec<atspi::Action>) -> Result<Vec<atspi::Action>> {
    if actions.len() > MAX_ACTIONS
        || actions.iter().any(|action| {
            [&action.name, &action.description, &action.keybinding]
                .iter()
                .any(|value| value.len() > MAX_METADATA_BYTES)
        })
    {
        return Err(anyhow!("AT-SPI action metadata exceeds its size budget"));
    }
    Ok(actions)
}

async fn states_from_proxy(proxy: &AccessibleProxy<'_>) -> Vec<String> {
    proxy
        .get_state()
        .await
        .map(state_labels)
        .unwrap_or_default()
}

async fn value_from_proxies(
    proxies: Option<&atspi::proxy::proxy_ext::Proxies<'_>>,
) -> Option<AccessibilityValue> {
    let value = proxies?.value().await.ok()?;
    Some(AccessibilityValue {
        current: value.current_value().await.ok()?,
        minimum: value.minimum_value().await.ok()?,
        maximum: value.maximum_value().await.ok()?,
        minimum_increment: value.minimum_increment().await.ok()?,
        text: optional_string(value.text().await.ok()),
    })
}

async fn text_from_proxies(
    proxies: Option<&atspi::proxy::proxy_ext::Proxies<'_>>,
) -> Option<AccessibilityText> {
    let text = proxies?.text().await.ok()?;
    let character_count = text.character_count().await.ok()?.max(0);
    let caret_offset = text.caret_offset().await.ok();
    let capped_count = character_count.min(MAX_TEXT_READBACK_CHARS);
    let content = if capped_count > 0 {
        text.get_text(0, capped_count).await.ok()
    } else {
        None
    };
    let oversized_content = content
        .as_ref()
        .is_some_and(|value| value.chars().count() > MAX_TEXT_READBACK_CHARS as usize);
    let content = bounded_text(content);
    let selection_count = text
        .get_nselections()
        .await
        .unwrap_or_default()
        .clamp(0, MAX_TEXT_SELECTIONS);
    let mut selections = Vec::new();
    for index in 0..selection_count {
        if let Ok((start_offset, end_offset)) = text.get_selection(index).await {
            selections.push(AccessibilityTextSelection {
                start_offset,
                end_offset,
            });
        }
    }

    Some(AccessibilityText {
        character_count,
        caret_offset,
        content,
        truncated: character_count > MAX_TEXT_READBACK_CHARS || oversized_content,
        selections,
    })
}

async fn supports_editable_text(proxies: Option<&atspi::proxy::proxy_ext::Proxies<'_>>) -> bool {
    let Some(proxies) = proxies else {
        return false;
    };
    proxies.editable_text().await.is_ok()
}

fn state_labels(state_set: StateSet) -> Vec<String> {
    state_set.iter().map(|state| state.to_string()).collect()
}

fn select_action_index(actions: &[atspi::Action], requested_action: Option<&str>) -> Result<i32> {
    if actions.is_empty() {
        return Err(anyhow!("element exposes no AT-SPI actions"));
    }

    if let Some(requested_action) = requested_action
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let requested_action = requested_action.to_ascii_lowercase();
        if let Some((index, _)) = actions.iter().enumerate().find(|(_, action)| {
            action.name.to_ascii_lowercase() == requested_action
                || action.description.to_ascii_lowercase() == requested_action
        }) {
            return Ok(index as i32);
        }

        if let Ok(index) = requested_action.parse::<usize>() {
            if index < actions.len() {
                return Ok(index as i32);
            }
        }

        return Err(anyhow!(
            "requested AT-SPI action was not found; available actions: {}",
            actions
                .iter()
                .map(|action| action.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    Ok(if actions.len() > 1 { 1 } else { 0 })
}

fn select_named_action_index(actions: &[atspi::Action], name: &str) -> Result<i32> {
    actions
        .iter()
        .position(|action| action.name.eq_ignore_ascii_case(name))
        .map(|index| index as i32)
        .ok_or_else(|| {
            anyhow!("cached AT-SPI action name is no longer available; refresh get_app_state")
        })
}

fn optional_string(value: Option<String>) -> Option<String> {
    value
        .filter(|value| value.len() <= MAX_METADATA_BYTES)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn bounded_text(value: Option<String>) -> Option<String> {
    value
        .filter(|value| value.chars().count() <= MAX_TEXT_READBACK_CHARS as usize)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn valid_object_ref_metadata(object_ref: &ObjectRefOwned) -> bool {
    object_ref.name_as_str().unwrap_or_default().len() + object_ref.path_as_str().len()
        <= MAX_METADATA_BYTES
}

fn object_ref_from_id(object_ref_id: &str) -> Result<ObjectRefOwned> {
    let (name, path) = split_object_ref_id(object_ref_id)?;
    let name = UniqueName::try_from(name.to_string())
        .with_context(|| format!("invalid AT-SPI bus name in object ref {object_ref_id}"))?;
    let path = ObjectPath::try_from(path.to_string())
        .with_context(|| format!("invalid AT-SPI object path in object ref {object_ref_id}"))?;
    Ok(ObjectRef::new_owned(name, path))
}

fn split_object_ref_id(object_ref_id: &str) -> Result<(&str, &str)> {
    let Some(path_start) = object_ref_id.find('/') else {
        return Err(anyhow!(
            "invalid AT-SPI object ref '{object_ref_id}'; expected ':bus/path'"
        ));
    };
    let (name, path) = object_ref_id.split_at(path_start);
    if name.is_empty() || path.is_empty() {
        return Err(anyhow!(
            "invalid AT-SPI object ref '{object_ref_id}'; expected ':bus/path'"
        ));
    }
    Ok((name, path))
}

fn object_ref_id(object_ref: &ObjectRefOwned) -> String {
    format!(
        "{}{}",
        object_ref.name_as_str().unwrap_or(""),
        object_ref.path_as_str()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_object_ref_id_separates_bus_name_and_path() {
        let (name, path) = split_object_ref_id(":1.42/org/a11y/atspi/accessible/7").unwrap();

        assert_eq!(name, ":1.42");
        assert_eq!(path, "/org/a11y/atspi/accessible/7");
    }

    #[test]
    fn select_action_index_uses_named_action() {
        let actions = vec![
            atspi::Action {
                name: "click".to_string(),
                description: "Clicks".to_string(),
                keybinding: String::new(),
            },
            atspi::Action {
                name: "show-menu".to_string(),
                description: "Shows menu".to_string(),
                keybinding: String::new(),
            },
        ];

        assert_eq!(select_action_index(&actions, Some("show-menu")).unwrap(), 1);
    }

    #[test]
    fn cached_action_name_is_revalidated_against_current_actions() {
        let mut actions = vec![atspi::Action {
            name: "show-menu".into(),
            description: "Click".into(),
            keybinding: String::new(),
        }];
        // Neither a stale index nor a matching description may invoke another action.
        assert!(select_named_action_index(&actions, "Click").is_err());
        actions.push(atspi::Action {
            name: "click".into(),
            description: String::new(),
            keybinding: String::new(),
        });
        assert_eq!(select_named_action_index(&actions, "Click").unwrap(), 1);
    }

    #[test]
    fn select_action_index_defaults_to_secondary_when_available() {
        let actions = vec![
            atspi::Action {
                name: "click".to_string(),
                description: String::new(),
                keybinding: String::new(),
            },
            atspi::Action {
                name: "show-menu".to_string(),
                description: String::new(),
                keybinding: String::new(),
            },
        ];

        assert_eq!(select_action_index(&actions, None).unwrap(), 1);
    }

    #[test]
    fn state_labels_serialize_in_bit_order() {
        let labels = state_labels(StateSet::new(atspi::State::Focused | atspi::State::Checked));

        assert_eq!(labels, vec!["checked".to_string(), "focused".to_string()]);
    }

    #[test]
    fn default_snapshot_limits_cover_deep_gtk4_trees() {
        // Nautilus 50 places file-list cells below depth 20 and can expose
        // more than 850 raw nodes. Keep the defaults above that known shape.
        assert_eq!(snapshot_limits(None, None), (1_000, 32));
    }

    #[test]
    fn app_name_needle_ignores_blank_values_and_normalizes_the_rest() {
        assert_eq!(app_name_needle(None), None);
        assert_eq!(app_name_needle(Some("")), None);
        assert_eq!(app_name_needle(Some("   ")), None);
        assert_eq!(
            app_name_needle(Some("Calculator")).as_deref(),
            Some("calculator")
        );
        assert_eq!(
            app_name_needle(Some(" :1.64/org/a11y/atspi/accessible/root ")).as_deref(),
            Some(":1.64/org/a11y/atspi/accessible/root")
        );
    }

    #[test]
    fn requested_snapshot_limits_remain_bounded() {
        assert_eq!(snapshot_limits(Some(0), Some(0)), (1, 0));
        assert_eq!(snapshot_limits(Some(10_000), Some(128)), (2_000, 64));
    }

    #[test]
    fn app_discovery_budget_is_independent_of_requested_tree_size() {
        assert_eq!(snapshot_child_read_budgets(1), (256, 2_048, 1));
    }

    #[test]
    fn traversal_attempts_and_queue_share_one_work_budget() {
        let mut traversal = BoundedTraversal::new(4);
        traversal.enqueue([1]);

        assert_eq!(traversal.pop(), Some(1));
        traversal.enqueue(2..=10_000);
        assert_eq!(traversal.queue, VecDeque::from([2, 3, 4]));

        assert_eq!(traversal.pop(), Some(2));
        traversal.enqueue(5..=10_000);
        assert_eq!(traversal.queue, VecDeque::from([3, 4]));
        assert_eq!(traversal.pop(), Some(3));
        assert_eq!(traversal.pop(), Some(4));
        assert_eq!(traversal.pop(), None);
        assert_eq!(traversal.attempted, 4);
        assert!(
            traversal.truncated(),
            "dropped enqueue items must be reported"
        );
    }

    #[test]
    fn traversal_that_drains_within_budget_is_not_truncated() {
        let mut traversal = BoundedTraversal::new(3);
        traversal.enqueue([1, 2, 3]);
        assert_eq!(traversal.pop(), Some(1));
        assert_eq!(traversal.pop(), Some(2));
        assert_eq!(traversal.pop(), Some(3));
        assert_eq!(traversal.pop(), None);
        assert!(
            !traversal.truncated(),
            "exactly max_items real nodes is complete"
        );
    }

    #[test]
    fn enqueue_at_zero_capacity_marks_the_traversal_truncated() {
        let mut traversal = BoundedTraversal::new(2);
        traversal.enqueue([1, 2]);
        assert_eq!(traversal.pop(), Some(1));
        assert_eq!(traversal.pop(), Some(2));
        assert!(!traversal.truncated(), "nothing dropped yet");
        // Capacity is zero; the offered item is dropped, which is the only
        // path that can mark truncation.
        traversal.enqueue([3]);
        assert!(traversal.queue.is_empty());
        assert_eq!(traversal.pop(), None);
        assert!(traversal.truncated());
    }

    #[test]
    fn child_count_is_clamped_before_indexed_reads() {
        assert_eq!(bounded_child_count(-1, 4), 0);
        assert_eq!(bounded_child_count(3, 4), 3);
        assert_eq!(bounded_child_count(i32::MAX, 4), 4);
    }

    #[tokio::test]
    async fn indexed_child_reads_consume_attempts_even_when_one_fails() {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut remaining_attempts = 3;
        let batch = fetch_indexed_up_to(100, 10, &mut remaining_attempts, {
            let calls = calls.clone();
            move |index| {
                let calls = calls.clone();
                async move {
                    calls.lock().unwrap().push(index);
                    if index == 1 {
                        Err(())
                    } else {
                        Ok(index)
                    }
                }
            }
        })
        .await;

        assert_eq!(batch.items, vec![0, 2]);
        assert_eq!(batch.attempted, 3);
        assert!(
            batch.incomplete,
            "100 reported children, 3 attempted: budget cut must be reported"
        );
        assert!(!batch.all_failed());
        assert_eq!(*calls.lock().unwrap(), vec![0, 1, 2]);
        assert_eq!(remaining_attempts, 0);

        let no_children = fetch_indexed_up_to(100, 10, &mut remaining_attempts, |_| async {
            Ok::<_, ()>(99)
        })
        .await;
        assert!(no_children.items.is_empty());
        assert_eq!(no_children.attempted, 0);
        assert!(!no_children.all_failed());

        let mut failed_attempts = 2;
        let all_failed =
            fetch_indexed_up_to(2, 2, &mut failed_attempts, |_| async { Err::<i32, _>(()) }).await;
        assert!(all_failed.all_failed());
    }

    struct MockBus;

    #[zbus::interface(name = "org.freedesktop.DBus")]
    impl MockBus {
        #[zbus(name = "GetConnectionUnixProcessID")]
        fn get_connection_unix_process_id(&self, _name: &str) -> u32 {
            std::process::id()
        }
    }

    struct MockAccessible {
        name: String,
        stalled: bool,
        children: Vec<ObjectRefOwned>,
    }

    #[zbus::interface(name = "org.a11y.atspi.Accessible")]
    impl MockAccessible {
        #[zbus(property)]
        async fn name(&self) -> &str {
            if self.stalled {
                futures_util::future::pending::<()>().await;
            }
            &self.name
        }

        #[zbus(property)]
        fn child_count(&self) -> i32 {
            self.children.len() as i32
        }

        fn get_child_at_index(&self, index: i32) -> ObjectRefOwned {
            self.children[index as usize].clone()
        }

        fn get_role_name(&self) -> &str {
            "application"
        }

        fn get_interfaces(&self) -> Vec<&str> {
            vec!["org.a11y.atspi.Accessible"]
        }
    }

    async fn mock_registry(apps: &[(&str, bool)]) -> (Connection, Connection) {
        let (service, client) = crate::atspi_transport::test_connection_pair()
            .await
            .unwrap();
        service
            .object_server()
            .at("/org/freedesktop/DBus", MockBus)
            .await
            .unwrap();
        let mut children = Vec::new();
        for (index, (name, stalled)) in apps.iter().enumerate() {
            let path = format!("/org/a11y/atspi/accessible/app{index}");
            children.push(ObjectRef::new_owned(
                service.unique_name().unwrap().clone(),
                ObjectPath::try_from(path.clone()).unwrap(),
            ));
            service
                .object_server()
                .at(
                    path,
                    MockAccessible {
                        name: (*name).to_string(),
                        stalled: *stalled,
                        children: Vec::new(),
                    },
                )
                .await
                .unwrap();
        }
        service
            .object_server()
            .at(
                "/org/a11y/atspi/accessible/root",
                MockAccessible {
                    name: "registry".to_string(),
                    stalled: false,
                    children,
                },
            )
            .await
            .unwrap();
        service
            .request_name("org.a11y.atspi.Registry")
            .await
            .unwrap();
        service.request_name("org.freedesktop.DBus").await.unwrap();
        (service, client)
    }

    #[tokio::test]
    async fn discovery_deadline_includes_connect_and_zero_limit_stays_io_free() {
        let started = tokio::time::Instant::now();
        let result = list_accessible_apps_with_connection(
            50,
            Duration::from_millis(30),
            futures_util::future::pending(),
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("deadline"));
        assert!(started.elapsed() < Duration::from_secs(1));
        let empty = list_accessible_apps_with_connection(
            0,
            Duration::from_millis(30),
            futures_util::future::pending(),
        )
        .await
        .unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn discovery_skips_stalled_provider_and_preserves_healthy_app() {
        let (_service, conn) = mock_registry(&[("wedged", true), ("Calculator", false)]).await;
        let started = tokio::time::Instant::now();
        let apps = list_accessible_apps_with_connection(50, DISCOVERY_TIMEOUT, async { Ok(conn) })
            .await
            .unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name.as_deref(), Some("Calculator"));
        assert_eq!(apps[0].role, "application");
        assert_eq!(apps[0].pid, Some(std::process::id()));
        assert!(apps[0].object_ref.ends_with("/app1"));
    }

    #[tokio::test]
    async fn discovery_total_deadline_returns_completed_summaries() {
        let (_service, conn) =
            mock_registry(&[("Calculator", false), ("wedged1", true), ("wedged2", true)]).await;
        let started = tokio::time::Instant::now();
        let apps = list_accessible_apps_with_connection(50, Duration::from_millis(450), async {
            Ok(conn)
        })
        .await
        .unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name.as_deref(), Some("Calculator"));
    }

    #[tokio::test]
    async fn discovery_rejects_oversized_metadata_without_truncating_identity() {
        let oversized = "x".repeat(MAX_METADATA_BYTES + 1);
        let (_service, conn) = mock_registry(&[(&oversized, false), ("Calculator", false)]).await;
        let apps = list_accessible_apps_with_connection(50, DISCOVERY_TIMEOUT, async { Ok(conn) })
            .await
            .unwrap();
        assert_eq!(apps.len(), 2);
        assert!(apps[0].name.is_none());
        assert_eq!(apps[1].name.as_deref(), Some("Calculator"));
        let json = serde_json::to_string(&apps).unwrap();
        assert!(!json.contains(&oversized));
    }

    struct MockActions(Vec<atspi::Action>);

    #[zbus::interface(name = "org.a11y.atspi.Action")]
    impl MockActions {
        fn get_actions(&self) -> Vec<atspi::Action> {
            self.0.clone()
        }

        async fn do_action(&self, index: i32) -> bool {
            tokio::time::sleep(Duration::from_millis(600)).await;
            index == 0
        }
    }

    fn mock_action(name: &str) -> atspi::Action {
        atspi::Action {
            name: name.to_string(),
            description: "Clicks".to_string(),
            keybinding: String::new(),
        }
    }

    #[tokio::test]
    async fn action_reader_rejects_excess_count_and_metadata_and_preserves_names() {
        let (service, conn) = crate::atspi_transport::test_connection_pair()
            .await
            .unwrap();
        service
            .object_server()
            .at(
                "/actions",
                MockActions(vec![mock_action("click"); MAX_ACTIONS + 1]),
            )
            .await
            .unwrap();
        let proxy = atspi::proxy::action::ActionProxy::builder(&conn)
            .destination(service.unique_name().unwrap())
            .unwrap()
            .path("/actions")
            .unwrap()
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await
            .unwrap();
        assert!(read_actions(&proxy).await.is_err());
        let iface = service
            .object_server()
            .interface::<_, MockActions>("/actions")
            .await
            .unwrap();
        iface.get_mut().await.0 = vec![mock_action(&"x".repeat(MAX_METADATA_BYTES + 1))];
        assert!(read_actions(&proxy).await.is_err());
        iface.get_mut().await.0 = vec![mock_action("click"); MAX_ACTIONS];
        let actions = read_actions(&proxy).await.unwrap();
        assert_eq!(actions.len(), MAX_ACTIONS);
        assert_eq!(select_named_action_index(&actions, "click").unwrap(), 0);
        assert_eq!(actions[0].description, "Clicks");
    }

    #[test]
    fn metadata_and_text_budgets_preserve_utf8_and_reject_instead_of_truncate() {
        assert_eq!(
            optional_string(Some(" Calculator ".into())).as_deref(),
            Some("Calculator")
        );
        assert!(optional_string(Some("x".repeat(MAX_METADATA_BYTES + 1))).is_none());
        let unicode = "🦀".repeat(MAX_TEXT_READBACK_CHARS as usize);
        assert_eq!(bounded_text(Some(unicode.clone())), Some(unicode));
        assert!(bounded_text(Some("🦀".repeat(MAX_TEXT_READBACK_CHARS as usize + 1))).is_none());
        let object =
            object_ref_from_id(&format!(":1.42/{}", "x".repeat(MAX_METADATA_BYTES))).unwrap();
        assert!(!valid_object_ref_metadata(&object));
        let object = object_ref_from_id(":1.42/org/a11y/atspi/accessible/7").unwrap();
        assert!(valid_object_ref_metadata(&object));
    }

    #[tokio::test]
    async fn snapshot_metadata_budget_stops_retention_and_reports_partial_tree() {
        let (_service, conn) = mock_registry(&[("Calculator", false), ("Terminal", false)]).await;
        let snapshot = snapshot_tree_on_connection(&conn, None, None, 10, 4, 100)
            .await
            .unwrap();
        assert_eq!(snapshot.nodes.len(), 1);
        assert_eq!(snapshot.nodes[0].name.as_deref(), Some("Calculator"));
        assert!(snapshot.truncated);
        let complete =
            snapshot_tree_on_connection(&conn, None, None, 10, 4, MAX_SNAPSHOT_METADATA_BYTES)
                .await
                .unwrap();
        assert_eq!(complete.nodes.len(), 2);
        assert!(!complete.truncated);
    }

    struct StalledRegistry;

    #[zbus::interface(name = "org.a11y.atspi.Accessible")]
    impl StalledRegistry {
        #[zbus(property)]
        async fn child_count(&self) -> i32 {
            futures_util::future::pending().await
        }
    }

    #[tokio::test]
    async fn discovery_deadline_also_covers_registry_reads() {
        let (service, conn) = mock_registry(&[]).await;
        service
            .object_server()
            .remove::<MockAccessible, _>("/org/a11y/atspi/accessible/root")
            .await
            .unwrap();
        service
            .object_server()
            .at("/org/a11y/atspi/accessible/root", StalledRegistry)
            .await
            .unwrap();
        let started = tokio::time::Instant::now();
        let result =
            list_accessible_apps_with_connection(50, Duration::from_millis(30), async { Ok(conn) })
                .await;
        assert!(result.unwrap_err().to_string().contains("deadline"));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    struct MutationInterfaces;

    #[zbus::interface(name = "org.a11y.atspi.Accessible")]
    impl MutationInterfaces {
        fn get_interfaces(&self) -> Vec<&str> {
            vec![
                "org.a11y.atspi.Accessible",
                "org.a11y.atspi.Action",
                "org.a11y.atspi.Value",
            ]
        }
    }

    struct MockValue(std::sync::Arc<std::sync::Mutex<f64>>);

    #[zbus::interface(name = "org.a11y.atspi.Value")]
    impl MockValue {
        #[zbus(property)]
        fn current_value(&self) -> f64 {
            *self.0.lock().unwrap()
        }

        #[zbus(property)]
        async fn set_current_value(&mut self, value: f64) {
            tokio::time::sleep(Duration::from_millis(600)).await;
            *self.0.lock().unwrap() = value;
        }
    }

    #[tokio::test]
    async fn mutations_wait_for_completion_beyond_the_observation_method_deadline() {
        let (service, conn) = crate::atspi_transport::test_mutation_connection_pair()
            .await
            .unwrap();
        service
            .object_server()
            .at("/actions", MutationInterfaces)
            .await
            .unwrap();
        service
            .object_server()
            .at("/actions", MockActions(vec![mock_action("click")]))
            .await
            .unwrap();
        let value = std::sync::Arc::new(std::sync::Mutex::new(0.0));
        service
            .object_server()
            .at("/actions", MockValue(value.clone()))
            .await
            .unwrap();
        let object_ref = format!("{}/actions", service.unique_name().unwrap());
        let started = tokio::time::Instant::now();
        let action = perform_action_on_connection(&conn, &object_ref, Some("click"), true)
            .await
            .unwrap();
        assert!(action.ok);
        assert_eq!(action.action_name.as_deref(), Some("click"));
        assert!(started.elapsed() >= Duration::from_millis(600));
        let result = set_element_value_on_connection(&conn, &object_ref, "42")
            .await
            .unwrap();
        assert!(matches!(
            result,
            ValueSetInvocation::Numeric { value: 42.0 }
        ));
        assert_eq!(*value.lock().unwrap(), 42.0);
    }
}
