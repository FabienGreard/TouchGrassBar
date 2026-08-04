use touchgrassbar_lib::sanitized::native_contract_schema;

fn main() {
    let schema = native_contract_schema();
    println!(
        "{}",
        serde_json::to_string_pretty(&schema).expect("native contract schema must serialize")
    );
}
