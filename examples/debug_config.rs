use anyhow::Result;
use reqwest::Client;
use std::env;

#[tokio::main]
async fn main() -> Result<()> {
    let client = Client::new();
    let url = "http://127.0.0.1:9090/configs";
    let mut request = client.get(url);

    let secret = env::var("MIHOMO_SECRET").unwrap_or_else(|_| "mihomo".to_string());
    request = request.bearer_auth(secret);

    let resp = request.send().await?.text().await?;
    println!("{}", resp);
    Ok(())
}