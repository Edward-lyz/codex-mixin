#![forbid(unsafe_code)]

mod cli;

#[tokio::main]
async fn main() {
    cli::entrypoint().await;
}
