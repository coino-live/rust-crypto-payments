use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::error::Error;

#[derive(Serialize)]
struct OrderPayload {
    Amount: f64,
    Callback: String,
    Description: String,
    Gate_ID: String,
    Create: bool,
}

pub struct CoinoLive {
    http_client: Client,
    api_key: String,
}

impl CoinoLive {
    pub fn new(api_key: String) -> CoinoLive {
        CoinoLive {
            http_client: Client::new(),
            api_key,
        }
    }

    pub async fn create_order(
        &self,
        amount: f64,
        callback_url: String,
        description: String,
        gate_id: String,
    ) -> Result<String, Box<dyn Error>> {
        let payload = OrderPayload {
            Amount: amount,
            Callback: callback_url,
            Description: description,
            Gate_ID: gate_id,
            Create: true,
        };

        let response = self
            .http_client
            .post("https://coino.live/api/v1/order")
            .header("x-api-key", &self.api_key)
            .json(&payload)
            .send()
            .await?;

        let text = response.text().await?;
        Ok(text)
    }

    pub fn redirect_payment(&self, order_id: &str) {
        let payment_url = format!("https://coino.live/orders/{}", order_id);
        if let Err(e) = open::that(payment_url) {
            eprintln!("Failed to open URL: {}", e);
        }
    }

    pub async fn verify_payment(&self, order_id: &str) -> Result<String, Box<dyn Error>> {
        let response = self
            .http_client
            .get(&format!("https://coino.live/api/v1/verify?order={}", order_id))
            .header("x-api-key", &self.api_key)
            .send()
            .await?;

        let text = response.text().await?;
        Ok(text)
    }
}
