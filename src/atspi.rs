//! Private bindings for the AT-SPI protocol surface used by this application.
//!
//! These replace the zbus-5-only atspi crates. Wire signatures and role/state
//! labels follow atspi 0.29 / atspi-common and atspi-proxies 0.13 (MIT).
//! See THIRD_PARTY_LICENSES/atspi-MIT.txt. No peer-to-peer discovery is performed.
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use zbus::{
    fdo::DBusProxy, names::UniqueName, proxy::CacheProperties, Connection, MatchRule,
    MessageStream, ObjectPath, OwnedObjectPath, Type,
};

mod roles;
pub(crate) use roles::Role;
include!("atspi/state_names.rs");

const NULL_PATH: &str = "/org/a11y/atspi/null";
const ROOT_PATH: &str = "/org/a11y/atspi/accessible/root";
const REGISTRY_NAME: &str = "org.a11y.atspi.Registry";
const ACTIVATE_EVENT: &str = "window:activate";
const ACTIVATE_MATCH: &str =
    "type='signal',interface='org.a11y.atspi.Event.Window',member='Activate'";

#[derive(Debug, Clone, Type)]
#[zbus(signature = "(so)")]
pub(crate) struct ObjectRefOwned {
    name: Option<UniqueName<'static>>,
    path: OwnedObjectPath,
}

impl ObjectRefOwned {
    pub(crate) fn new(name: UniqueName<'static>, path: ObjectPath<'static>) -> Self {
        Self {
            name: (path.as_str() != NULL_PATH).then_some(name),
            path: path.into(),
        }
    }

    pub(crate) fn name_as_str(&self) -> Option<&str> {
        self.name.as_ref().map(UniqueName::as_str)
    }

    pub(crate) fn path_as_str(&self) -> &str {
        self.path.as_str()
    }

    pub(crate) async fn as_accessible_proxy(
        &self,
        conn: &Connection,
    ) -> zbus::Result<AccessibleProxy<'static>> {
        let name = self
            .name
            .clone()
            .ok_or_else(|| zbus::Error::Failure("null AT-SPI object reference".into()))?;
        AccessibleProxy::builder(conn)
            .destination(name)
            .path(self.path.clone())
            .cache_properties(CacheProperties::No)
            .build()
            .await
    }
}

impl<'de> Deserialize<'de> for ObjectRefOwned {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (name, path): (String, OwnedObjectPath) = Deserialize::deserialize(deserializer)?;
        // The null path is a valid response from GTK3 for a missing child.
        // Every other object must have a valid unique bus owner, including on
        // malformed replies: never create a destination-less proxy or panic.
        let name = if path.as_str() == NULL_PATH {
            None
        } else {
            Some(UniqueName::try_from(name).map_err(serde::de::Error::custom)?)
        };
        Ok(Self { name, path })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct Action {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) keybinding: String,
}

#[derive(Debug, Clone, Type)]
#[zbus(signature = "au")]
pub(crate) struct StateSet(u64);

impl StateSet {
    pub(crate) fn from_words(words: Vec<u32>) -> zbus::Result<Self> {
        let [low, high]: [u32; 2] = words
            .try_into()
            .map_err(|_| zbus::Error::Failure("AT-SPI state requires two words".into()))?;
        let bits = u64::from(low) | (u64::from(high) << 32);
        if bits >> STATE_NAMES.len() != 0 {
            return Err(zbus::Error::Failure("unknown AT-SPI state bits".into()));
        }
        Ok(Self(bits))
    }
    pub(crate) fn focused(&self) -> bool {
        self.0 & (1 << 12) != 0
    }
    pub(crate) fn labels(&self) -> Vec<String> {
        STATE_NAMES
            .iter()
            .enumerate()
            .filter(|(i, _)| self.0 & (1 << i) != 0)
            .map(|(_, name)| (*name).to_owned())
            .collect()
    }
}

impl<'de> Deserialize<'de> for StateSet {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_words(Vec::<u32>::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[zbus::proxy(
    interface = "org.a11y.atspi.Accessible",
    default_path = "/org/a11y/atspi/accessible/root"
)]
pub(crate) trait Accessible {
    fn get_child_at_index(&self, index: i32) -> zbus::Result<ObjectRefOwned>;
    fn get_interfaces(&self) -> zbus::Result<Vec<String>>;
    fn get_role(&self) -> zbus::Result<Role>;
    fn get_role_name(&self) -> zbus::Result<String>;
    fn get_state(&self) -> zbus::Result<StateSet>;
    #[zbus(property)]
    fn child_count(&self) -> zbus::Result<i32>;
    #[zbus(property)]
    fn description(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn name(&self) -> zbus::Result<String>;
}

#[zbus::proxy(interface = "org.a11y.atspi.Action")]
pub(crate) trait ActionInterface {
    fn get_actions(&self) -> zbus::Result<Vec<Action>>;
    fn do_action(&self, index: i32) -> zbus::Result<bool>;
}

#[zbus::proxy(interface = "org.a11y.atspi.Component")]
pub(crate) trait Component {
    fn get_extents(&self, coord_type: u32) -> zbus::Result<(i32, i32, i32, i32)>;
}

#[zbus::proxy(interface = "org.a11y.atspi.Value")]
pub(crate) trait Value {
    #[zbus(property)]
    fn current_value(&self) -> zbus::Result<f64>;
    #[zbus(property)]
    fn set_current_value(&self, value: f64) -> zbus::Result<()>;
    #[zbus(property)]
    fn minimum_value(&self) -> zbus::Result<f64>;
    #[zbus(property)]
    fn maximum_value(&self) -> zbus::Result<f64>;
    #[zbus(property)]
    fn minimum_increment(&self) -> zbus::Result<f64>;
    #[zbus(property)]
    fn text(&self) -> zbus::Result<String>;
}

#[zbus::proxy(interface = "org.a11y.atspi.Text")]
pub(crate) trait Text {
    fn get_text(&self, start_offset: i32, end_offset: i32) -> zbus::Result<String>;
    #[zbus(name = "GetNSelections")]
    fn get_nselections(&self) -> zbus::Result<i32>;
    fn get_selection(&self, selection_num: i32) -> zbus::Result<(i32, i32)>;
    #[zbus(property)]
    fn character_count(&self) -> zbus::Result<i32>;
    #[zbus(property)]
    fn caret_offset(&self) -> zbus::Result<i32>;
}

#[zbus::proxy(interface = "org.a11y.atspi.EditableText")]
pub(crate) trait EditableText {
    fn set_text_contents(&self, value: &str) -> zbus::Result<bool>;
}

pub(crate) struct Proxies<'a> {
    interfaces: Vec<String>,
    proxy: zbus::Proxy<'a>,
}

impl<'a> AccessibleProxy<'a> {
    pub(crate) async fn proxies(&self) -> zbus::Result<Proxies<'a>> {
        Ok(Proxies {
            interfaces: self.get_interfaces().await?,
            proxy: self.inner().clone(),
        })
    }
}

macro_rules! interface_proxy {
    ($method:ident, $proxy:ident, $interface:literal) => {
        pub(crate) async fn $method(&self) -> zbus::Result<$proxy<'a>> {
            if !self
                .interfaces
                .iter()
                .any(|i| i == concat!("org.a11y.atspi.", $interface))
            {
                return Err(zbus::Error::Failure(
                    concat!("AT-SPI interface not available: ", $interface).into(),
                ));
            }
            $proxy::builder(self.proxy.connection())
                .destination(self.proxy.destination().clone())
                .path(self.proxy.path().clone())
                .cache_properties(CacheProperties::No)
                .build()
                .await
        }
    };
}
impl<'a> Proxies<'a> {
    interface_proxy!(action, ActionInterfaceProxy, "Action");
    interface_proxy!(component, ComponentProxy, "Component");
    interface_proxy!(value, ValueProxy, "Value");
    interface_proxy!(text, TextProxy, "Text");
    interface_proxy!(editable_text, EditableTextProxy, "EditableText");
}

#[zbus::proxy(
    interface = "org.a11y.Bus",
    default_service = "org.a11y.Bus",
    default_path = "/org/a11y/bus"
)]
trait Bus {
    fn get_address(&self) -> zbus::Result<String>;
}

#[zbus::proxy(
    interface = "org.a11y.atspi.Registry",
    default_service = "org.a11y.atspi.Registry",
    default_path = "/org/a11y/atspi/registry"
)]
trait Registry {
    fn register_event(&self, event: &str) -> zbus::Result<()>;
    fn deregister_event(&self, event: &str) -> zbus::Result<()>;
}

pub(crate) struct AccessibilityConnection {
    connection: Connection,
}
impl AccessibilityConnection {
    pub(crate) async fn new() -> zbus::Result<Self> {
        let session = Connection::session().await?;
        let address = BusProxy::new(&session).await?.get_address().await?;
        Self::from_address(&address).await
    }
    pub(crate) async fn from_address(address: &str) -> zbus::Result<Self> {
        let connection = zbus::connection::Builder::address(address).build().await?;
        Ok(Self { connection })
    }
    pub(crate) fn connection(&self) -> &Connection {
        &self.connection
    }
    pub(crate) async fn root_accessible_on_registry(
        &self,
    ) -> zbus::Result<AccessibleProxy<'static>> {
        AccessibleProxy::builder(&self.connection)
            .destination(REGISTRY_NAME)
            .path(ROOT_PATH)
            .cache_properties(CacheProperties::No)
            .build()
            .await
    }
    pub(crate) async fn register_activation_listener(&self) -> zbus::Result<()> {
        RegistryProxy::new(&self.connection)
            .await?
            .register_event(ACTIVATE_EVENT)
            .await?;
        DBusProxy::new(&self.connection)
            .await?
            .add_match_rule(MatchRule::try_from(ACTIVATE_MATCH)?)
            .await
            .map_err(Into::into)
    }
    pub(crate) async fn deregister_activation_listener(&self) -> zbus::Result<()> {
        RegistryProxy::new(&self.connection)
            .await?
            .deregister_event(ACTIVATE_EVENT)
            .await?;
        DBusProxy::new(&self.connection)
            .await?
            .remove_match_rule(MatchRule::try_from(ACTIVATE_MATCH)?)
            .await
            .map_err(Into::into)
    }
    pub(crate) fn event_stream(&self) -> impl Stream<Item = zbus::Result<zbus::Message>> {
        // Contents are intentionally discarded by the guard. Keep its stream
        // restricted to AT-SPI signals, as with the previous wrapper.
        MessageStream::from(&self.connection).filter_map(|result| {
            std::future::ready({
                match result {
                    Ok(message)
                        if message.message_type() == zbus::message::Type::Signal
                            && message
                                .header()
                                .interface()
                                .is_some_and(|i| i.starts_with("org.a11y.atspi")) =>
                    {
                        Some(Ok(message))
                    }
                    Err(error) => Some(Err(error)),
                    _ => None,
                }
            })
        })
    }
}

#[cfg(test)]
mod tests;
