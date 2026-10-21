// Scratch probe: surface the real AT-SPI connection error chain.
use zbus::{Connection, OwnedObjectPath, Proxy};

async fn probe() -> zbus::Result<usize> {
    let session = Connection::session().await?;
    let launcher = Proxy::new(&session, "org.a11y.Bus", "/org/a11y/bus", "org.a11y.Bus").await?;
    let address: String = launcher.call("GetAddress", &()).await?;
    let connection = zbus::connection::Builder::address(address.as_str())
        .build()
        .await?;
    let root = Proxy::new(
        &connection,
        "org.a11y.atspi.Registry",
        "/org/a11y/atspi/accessible/root",
        "org.a11y.atspi.Accessible",
    )
    .await?;
    let children: Vec<(String, OwnedObjectPath)> = root.call("GetChildren", &()).await?;
    Ok(children.len())
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    println!(
        "AT_SPI_BUS_ADDRESS={:?}",
        std::env::var("AT_SPI_BUS_ADDRESS")
    );
    println!(
        "DBUS_SESSION_BUS_ADDRESS={:?}",
        std::env::var("DBUS_SESSION_BUS_ADDRESS")
    );
    match probe().await {
        Ok(count) => println!("connected OK; registry children: {count}"),
        Err(error) => println!("AT-SPI probe error: {error:#?}"),
    }
}
