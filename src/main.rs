#![forbid(unsafe_code)]

mod cli;
#[path = "../tui/mod.rs"]
mod tui;

#[tokio::main]
async fn main() {
    cli::entrypoint(|start, installed_cli_path| {
        let start_page = match start {
            cli::InteractiveStart::Dashboard => tui::StartPage::Dashboard,
            cli::InteractiveStart::Setup => tui::StartPage::Setup,
        };
        Box::pin(tui::run(start_page, installed_cli_path))
    })
    .await;
}
