use rtnetlink::{new_connection, LinkBridge};
use tokio;

#[tokio::main]
async fn main() {
    let (connection, handle, _) = new_connection().unwrap();

    tokio::spawn(connection);
    handle
        .link()
        .add(LinkBridge::new("br0").build())
        .execute()
        .await
        .unwrap();
}
