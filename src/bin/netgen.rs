use netgen::plugins::Config;
use netgen::topology::Topology;

#[tokio::main]
async fn main() {
    let config = match Config::from_yaml_file("./assets/config.yml") {
        Ok(config) => Some(config),
        Err(_err) => None,
    };
    let _topo = Topology::from_yaml_file("./assets/sample-top.yml", config).await;
}
