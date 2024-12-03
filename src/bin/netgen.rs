use netgen::plugins::yaml_parse_config_contents;
use netgen::topology::Topology;
use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read;
use yaml_rust2::YamlLoader;

#[tokio::main]
async fn main() {
    let t = Topology::from_yaml_file("./assets/sample-top.yml").await;
    //let mut f = File::open("./assets/config.yaml").unwrap();
    //let mut contents = String::new();
    //let _ = f.read_to_string(&mut contents);
    //let d = YamlLoader::load_from_str(contents.as_str()).unwrap();
    //
    //for x in d {
    //    let config = yaml_parse_config_contents(&x);
    //    println!("{:#?}", config);
    //}
}
