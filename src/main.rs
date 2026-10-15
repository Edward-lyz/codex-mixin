#![forbid(unsafe_code)]

mod cli;
mod ducx_auth_carrier;
#[path = "../panel/mod.rs"]
mod panel;

#[tokio::main]
async fn main() {
    cli::entrypoint(|start, installed_cli_path| {
        let (start_page, bind) = match start {
            cli::InteractiveStart::Dashboard { bind } => (panel::StartPage::Dashboard, bind),
            cli::InteractiveStart::Setup => (panel::StartPage::Setup, None),
        };
        Box::pin(panel::run(start_page, bind, installed_cli_path))
    })
    .await;
}
