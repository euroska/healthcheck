extern crate tokio;
use hyper::{Client, StatusCode, Uri};
use serde::Deserialize;
use std::{error::Error, fs, time::Duration};
use teloxide::{prelude::*, types::ChatId};
use tokio::time::sleep;
use tracing::{error, info};
use tracing_subscriber::prelude::*;

const CONFIG_ENV: &str = "HEALTHCHECK_CONFIG";
const CONFIG_VAL: &str = "healthcheck.toml";

#[derive(Deserialize, Clone)]
struct Config {
    telegram_token: String,
    telegram_chat_id: i64,
    check_interval_success: u64,
    check_interval_fail: u64,
    notify_failures: u64,
    rereport: u64,
    addresses: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    if std::env::var(CONFIG_ENV).is_err() {
        std::env::set_var(CONFIG_ENV, CONFIG_VAL);
    }

    let path = std::env::var(CONFIG_ENV).unwrap();
    let contents = fs::read_to_string(path)?;
    let config: Config = toml::from_str(contents.as_ref()).unwrap();

    let fmt_layer = tracing_subscriber::fmt::layer().with_test_writer();
    let rust_tls = tracing_subscriber::filter::Targets::new()
        .with_target("rustls", tracing::Level::ERROR)
        .with_default(tracing_subscriber::fmt::Subscriber::DEFAULT_MAX_LEVEL);

    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(rust_tls)
        .init();

    let bot = Bot::new(config.telegram_token.clone());

    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .https_or_http()
        .enable_http1()
        .build();

    let client = Client::builder().build::<_, hyper::Body>(https);

    let mut handles = vec![];
    let address = config.addresses.clone();

    for u in address {
        let bot = bot.clone();
        let client = client.clone();
        let config = config.clone();

        handles.push(tokio::spawn(async move {
            check(
                string_to_static_str(u.to_string()),
                bot.clone(),
                config.clone(),
                client.clone(),
            )
            .await
        }));
    }
    futures::future::join_all(handles).await;
    Result::Ok(())
}

async fn check<'a>(
    url: &str,
    bot: Bot,
    config: Config,
    client: Client<hyper_rustls::HttpsConnector<hyper::client::HttpConnector>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut number_of_fail: u64 = 0;
    let mut number_of_success: u64 = 0;
    let mut fail_in_row: u64 = 0;

    match url.parse::<Uri>() {
        Ok(uri) => loop {
            let mut message: Option<String> = None;

            match client.get(uri.clone()).await {
                Result::Ok(response) if (response.status() == StatusCode::OK) => {
                    number_of_success += 1;

                    if fail_in_row > 0 {
                        fail_in_row = 0;
                        message = Some(format!("{} Recovered", url));
                    } else {
                        info!("Check {} OK", url);
                    }
                }
                Result::Ok(response) => {
                    number_of_fail += 1;
                    fail_in_row += 1;
                    message = Some(format!(
                        "{}: status {}, failures: {}, succes: {}",
                        url,
                        response.status(),
                        number_of_fail,
                        number_of_success
                    ));
                }
                Result::Err(error) => {
                    number_of_fail += 1;
                    fail_in_row += 1;
                    message = Some(format!(
                        "{}: {}, failures: {}, succes: {}",
                        url, error, number_of_fail, number_of_success
                    ));
                }
            };

            if message.is_none() {
                sleep(Duration::from_millis(config.check_interval_success)).await;
            } else {
                if fail_in_row == config.notify_failures || (fail_in_row % config.rereport) == 0 {
                    let message = message.unwrap();
                    info!("{}", message);
                    match bot
                        .send_message(ChatId(config.telegram_chat_id), message)
                        .send()
                        .await
                    {
                        Result::Err(error) => {
                            error!("[{}]: telegram error {}", url, error);
                        }
                        _ => {}
                    }
                }
                sleep(Duration::from_millis(config.check_interval_fail)).await;
            }
        },
        Err(_) => {
            error!("Bad URL format: {}", url);
        }
    }

    Ok(())
}

fn string_to_static_str(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}
