use codex_mixin::application::routing::{self, ModelRoute};
use serde_json::json;

pub(super) fn list_routes(json_output: bool) -> anyhow::Result<()> {
    let routes = routing::list_routes()?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "routes": routes }))?
        );
    } else if routes.is_empty() {
        println!("no model routes configured");
    } else {
        for entry in routes {
            println!("{} [{}]", entry.model, entry.route.kind());
        }
    }
    Ok(())
}

pub(super) fn get_route(model: &str, json_output: bool) -> anyhow::Result<()> {
    let route = routing::get_route(model)?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "route": route }))?
        );
    } else if let Some(route) = route {
        println!("{}", serde_json::to_string_pretty(&route)?);
    } else {
        anyhow::bail!("model route not found: {model}");
    }
    Ok(())
}

pub(super) fn set_route(route_json: &str, replace_model: Option<&str>) -> anyhow::Result<()> {
    let route: ModelRoute = serde_json::from_str(route_json)?;
    let model = routing::set_route(route, replace_model)?;
    println!("model route saved: {model}");
    Ok(())
}

pub(super) fn delete_route(model: &str) -> anyhow::Result<()> {
    let model = routing::delete_route(model)?;
    println!("model route deleted: {model}");
    Ok(())
}
