use touchgrassbar_lib::sanitized::native_contract_export;

fn main() {
    let contract = native_contract_export();
    println!(
        "{}",
        serde_json::to_string_pretty(&contract).expect("native contract must serialize")
    );
}
