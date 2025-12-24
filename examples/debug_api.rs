use anyhow::Result;
use reqwest::Client;

#[tokio::main]
async fn main() -> Result<()> {
    let client = Client::new();
    let url = "http://127.0.0.1:9090/proxies";
    let secret = "mihomo";

    println!("Trying with Bearer auth...");
    let resp1 = client
        .get(url)
        .bearer_auth(secret)
        .send()
        .await?
        .text()
        .await?;
    println!("Response 1: {}", resp1);

    println!("\nTrying with Authorization: secret...");
    let resp2 = client
        .get(url)
        .header("Authorization", secret)
        .send()
        .await?
        .text()
        .await?;
    println!("Response 2: {}", resp2);

    Ok(())
}
