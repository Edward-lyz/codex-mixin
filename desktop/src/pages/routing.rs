//! Routing page: failover, fusion and schedule routes. The editor works on
//! the saved route JSON so fields it does not show are preserved; the CLI
//! validates and stores the result.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::searchable_list::SearchableListItem;
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IndexPath, Sizable as _, StyledExt as _,
    WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    Anchor, Animation, AnimationExt as _, AnyElement, App, AppContext as _, Bounds, Context,
    ElementId, Entity, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, PathBuilder,
    Pixels, Point, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, WeakEntity, Window, canvas, div, fill, point, prelude::FluentBuilder as _, px,
    size,
};
use rust_i18n::t;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::{is_busy, placeholder, run_change, run_change_then};
use crate::store::{Change, Store, query};
use crate::theme;
use crate::tr;
use crate::widgets::{pill, row_divider, section, setting_row};

/// Virtual models created by fusion and schedule routes; they cannot be the
/// target of another route.
const VIRTUAL_PREFIX: &str = "auto/";
const MAX_FALLBACKS: usize = 4;
const TIMEOUTS_SECONDS: [u64; 5] = [30, 60, 120, 300, 600];
const SLOT_STEP_MINUTES: u64 = 30;
const DAY_MINUTES: u64 = 24 * 60;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Failover,
    Fusion,
    Schedule,
}

impl Kind {
    fn parse(value: &str) -> Self {
        match value {
            "fusion" => Kind::Fusion,
            "schedule" => Kind::Schedule,
            _ => Kind::Failover,
        }
    }

    fn id(self) -> &'static str {
        match self {
            Kind::Failover => "failover",
            Kind::Fusion => "fusion",
            Kind::Schedule => "schedule",
        }
    }

    fn title(self) -> SharedString {
        match self {
            Kind::Failover => tr!("routing.kind.failover"),
            Kind::Fusion => tr!("routing.kind.fusion"),
            Kind::Schedule => tr!("routing.kind.schedule"),
        }
    }

    fn icon(self) -> IconName {
        match self {
            Kind::Failover => IconName::Shield,
            Kind::Fusion => IconName::Layers,
            Kind::Schedule => IconName::Clock,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
struct RouteList {
    routes: Vec<Value>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct RouteModels {
    models: Vec<RouteModel>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
struct RouteModel {
    id: String,
    #[serde(default)]
    display_name: Option<String>,
}

impl SearchableListItem for RouteModel {
    type Value = String;

    fn title(&self) -> SharedString {
        self.display_name.as_deref().unwrap_or(&self.id).into()
    }

    fn value(&self) -> &Self::Value {
        &self.id
    }

    fn matches(&self, query: &str) -> bool {
        let query = query.to_lowercase();
        self.id.to_lowercase().contains(&query)
            || self
                .display_name
                .as_ref()
                .is_some_and(|name| name.to_lowercase().contains(&query))
    }

    fn render(&self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let name = self.display_name.as_ref().filter(|name| *name != &self.id);
        v_flex()
            .h_8()
            .min_w_0()
            .flex_1()
            .justify_center()
            .when_some(name, |row, name| {
                row.child(div().truncate().child(SharedString::from(name.clone())))
            })
            .child(
                div()
                    .truncate()
                    .font_family(cx.theme().mono_font_family.clone())
                    .when(name.is_some(), |id| {
                        id.text_xs().text_color(cx.theme().colors.muted_foreground)
                    })
                    .child(SharedString::from(self.id.clone())),
            )
    }
}

struct Draft {
    kind: Kind,
    /// Entry model of the saved route being edited.
    replace_model: Option<String>,
    payload: Map<String, Value>,
    dirty: bool,
}

impl Draft {
    fn text(&self, field: &str) -> Option<String> {
        self.payload
            .get(field)
            .and_then(Value::as_str)
            .map(str::to_owned)
    }

    fn list(&self, field: &str) -> Vec<String> {
        self.payload
            .get(field)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn set(&mut self, field: &str, value: Value) {
        self.payload.insert(field.to_owned(), value);
        self.dirty = true;
    }
}

pub struct RoutingPage {
    routes: Option<Vec<Value>>,
    models: Arc<Vec<RouteModel>>,
    error: Option<SharedString>,
    selected: Option<String>,
    draft: Option<Draft>,
    name: Entity<InputState>,
    /// Whether the failover flow previews a failing entry model.
    simulating: bool,
    revision: u64,
    _subscriptions: Vec<Subscription>,
}

fn route_model(route: &Value) -> String {
    route
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn route_kind(route: &Value) -> Kind {
    Kind::parse(
        route
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )
}

fn format_minute(minute: u64) -> String {
    format!("{:02}:{:02}", minute / 60, minute % 60)
}

impl RoutingPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("routing.name_placeholder")));
        let name_subscription = cx.subscribe(&name, |page, state, event, cx| {
            if !matches!(event, InputEvent::Change) {
                return;
            }
            let value = state.read(cx).value().trim().to_owned();
            if let Some(draft) = &mut page.draft {
                draft.set("id", Value::String(value));
                cx.notify();
            }
        });
        let store = Store::global(cx);
        let store_subscription = cx.observe(&store, |page, store: Entity<Store>, cx| {
            let revision = store.read(cx).revision;
            if revision != page.revision {
                page.revision = revision;
                page.reload(cx);
            }
        });
        let mut page = Self {
            routes: None,
            models: Arc::default(),
            error: None,
            selected: None,
            draft: None,
            name,
            simulating: false,
            revision: store.read(cx).revision,
            _subscriptions: vec![name_subscription, store_subscription],
        };
        page.reload(cx);
        page
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let routes = query::<RouteList>(vec!["route".into(), "list".into(), "--json".into()], cx);
        let models =
            query::<RouteModels>(vec!["route".into(), "models".into(), "--json".into()], cx);
        cx.spawn(async move |page, cx| {
            let routes = routes.await;
            let models = models.await;
            page.update(cx, |page, cx| {
                match (routes, models) {
                    (Ok(routes), Ok(models)) => {
                        page.error = None;
                        let models: Vec<_> = models
                            .models
                            .into_iter()
                            .filter(|model| !model.id.starts_with(VIRTUAL_PREFIX))
                            .collect();
                        if *page.models != models {
                            page.models = Arc::new(models);
                        }
                        page.routes = Some(routes.routes);
                        let clean = page.draft.as_ref().is_none_or(|draft| !draft.dirty);
                        if clean {
                            page.draft = None;
                        }
                    }
                    (Err(error), _) | (_, Err(error)) => page.error = Some(error.message.into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn model_label(&self, id: &str) -> SharedString {
        self.models
            .iter()
            .find(|model| model.id == id)
            .and_then(|model| model.display_name.clone())
            .unwrap_or_else(|| id.to_owned())
            .into()
    }

    fn select(&mut self, route: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let model = route_model(route);
        let mut payload = route.as_object().cloned().unwrap_or_default();
        for transient in ["model", "enabled", "missing_models", "activity", "kind"] {
            payload.remove(transient);
        }
        let name = payload
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        self.name
            .update(cx, |input, cx| input.set_value(name, window, cx));
        self.draft = Some(Draft {
            kind: route_kind(route),
            replace_model: Some(model.clone()),
            payload,
            dirty: false,
        });
        self.selected = Some(model);
        cx.notify();
    }

    fn create(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        let first = self
            .models
            .first()
            .map(|model| model.id.clone())
            .unwrap_or_default();
        let second = self
            .models
            .get(1)
            .map_or_else(|| first.clone(), |model| model.id.clone());
        let (name, payload) = match kind {
            Kind::Failover => (
                String::new(),
                json!({ "source": first, "fallbacks": [second] }),
            ),
            Kind::Fusion => (
                "fusion".to_owned(),
                json!({ "id": "fusion", "panel_models": [first, second], "judge_model": first, "final_model": second }),
            ),
            Kind::Schedule => (
                "schedule".to_owned(),
                json!({
                    "id": "schedule",
                    "final_model": first,
                    "panel_models": [],
                    "judge_model": first,
                    "time_routes": [{ "start_minute": 9 * 60, "end_minute": 18 * 60, "model": second }],
                }),
            ),
        };
        self.name
            .update(cx, |input, cx| input.set_value(name, window, cx));
        self.draft = Some(Draft {
            kind,
            replace_model: None,
            payload: payload.as_object().cloned().unwrap_or_default(),
            dirty: true,
        });
        self.selected = None;
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = &mut self.draft else {
            return;
        };
        let mut payload = draft.payload.clone();
        payload.insert("kind".to_owned(), Value::String(draft.kind.id().to_owned()));
        if payload
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(str::is_empty)
        {
            payload.remove("id");
        }
        let mut args = vec![
            "route".to_owned(),
            "set".to_owned(),
            "--route-json".to_owned(),
            Value::Object(payload).to_string(),
        ];
        if let Some(previous) = &draft.replace_model {
            args.extend(["--replace-model".to_owned(), previous.clone()]);
        }
        let saved = run_change_then(Change::new(t!("routing.save_label"), args).applied(), cx);
        let editing = self.selected.clone();
        cx.spawn(async move |page, cx| {
            if saved.await {
                page.update(cx, |page, cx| {
                    // Leave a route the user opened while the save ran.
                    if page.selected == editing {
                        page.draft = None;
                        page.selected = None;
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn update_draft(view: &WeakEntity<Self>, cx: &mut App, edit: impl FnOnce(&mut Draft)) {
        view.update(cx, |page, cx| {
            if let Some(draft) = &mut page.draft {
                edit(draft);
                cx.notify();
            }
        })
        .ok();
    }
}

impl RoutingPage {
    /// A searchable picker for a model in one position of the route.
    fn model_picker(
        &self,
        id: impl Into<ElementId>,
        current: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
        on_pick: impl Fn(&mut Draft, String) + 'static,
    ) -> AnyElement {
        let id = id.into();
        let current_index = self
            .models
            .iter()
            .position(|model| Some(&model.id) == current.as_ref())
            .map(|index| IndexPath::default().row(index));
        let view = cx.entity().downgrade();
        let picker = window.use_keyed_state(id.clone(), cx, |window, cx| {
            let state = cx.new(|cx| {
                SelectState::new(
                    SearchableVec::new(self.models.as_ref().clone()),
                    current_index,
                    window,
                    cx,
                )
                .searchable(true)
            });
            let subscription = window.subscribe(&state, cx, move |_, event, _, cx| {
                if let SelectEvent::Confirm(Some(model)) = event {
                    Self::update_draft(&view, cx, |draft| on_pick(draft, model.clone()));
                }
            });
            (state, self.models.clone(), subscription)
        });
        let (state, catalog, _) = picker.read(cx);
        let state = state.clone();
        let catalog_changed = !Arc::ptr_eq(catalog, &self.models);
        let available = current.as_ref().filter(|_| current_index.is_some());
        if catalog_changed || state.read(cx).selected_value() != available {
            if catalog_changed {
                picker.update(cx, |(_, catalog, _), _| *catalog = self.models.clone());
            }
            state.update(cx, |state, cx| {
                if catalog_changed {
                    state.set_items(SearchableVec::new(self.models.as_ref().clone()), window, cx);
                }
                if let Some(model) = &current {
                    state.set_selected_value(model, window, cx);
                } else {
                    state.set_selected_index(None, window, cx);
                }
            });
        }
        let placeholder = current.map_or_else(|| tr!("routing.choose_model"), SharedString::from);
        div()
            .h_7()
            .w_full()
            .min_w_0()
            .max_w_64()
            .child(
                Select::new(&state)
                    .id(id)
                    .small()
                    .placeholder(placeholder)
                    .accessibility_label(t!("routing.choose_model"))
                    .search_placeholder(t!("routing.search_models"))
                    .menu_width(px(360.0))
                    .menu_max_h(px(360.0))
                    .empty(|_, cx| {
                        div()
                            .p_3()
                            .text_sm()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child(t!("models.no_results"))
                    }),
            )
            .into_any_element()
    }

    /// The list's minus button: discards a route that was never saved, or
    /// deletes the saved route open in the editor after a confirmation.
    fn remove_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = &self.draft else {
            return;
        };
        let Some(model) = draft.replace_model.clone() else {
            self.draft = None;
            self.selected = None;
            cx.notify();
            return;
        };
        let view = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let model = model.clone();
            let view = view.clone();
            dialog
                .title(tr!("routing.delete_title"))
                .description(tr!("routing.delete_detail", model = model))
                .confirm()
                .ok_text(t!("routing.delete"))
                .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                .cancel_text(t!("common.cancel"))
                .on_ok(move |_, _, cx| {
                    // Close the editor so unsaved edits cannot recreate the
                    // deleted route.
                    view.update(cx, |page, cx| {
                        page.draft = None;
                        page.selected = None;
                        cx.notify();
                    })
                    .ok();
                    run_change(
                        Change::new(
                            t!("routing.delete_label"),
                            ["route", "delete", "--model", model.as_str()],
                        )
                        .applied(),
                        cx,
                    );
                    true
                })
        });
    }

    fn render_list(&self, routes: &[Value], cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let can_remove = self.draft.is_some() && !is_busy(cx);
        let view = cx.entity().downgrade();
        let rows = routes.iter().map(|route| {
            let model = route_model(route);
            let kind = route_kind(route);
            let enabled = route
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let missing = route
                .get("missing_models")
                .and_then(Value::as_array)
                .is_some_and(|items| !items.is_empty());
            let selected = self.selected.as_ref() == Some(&model);
            let target = route.clone();
            let toggle_model = model.clone();
            h_flex()
                .id(ElementId::Name(format!("route-{model}").into()))
                .gap_2p5()
                .px_2()
                .py_1p5()
                .rounded_lg()
                .border_1()
                .border_color(gpui_kit::transparent_black())
                .cursor_pointer()
                .when(selected, |row| {
                    row.bg(colors.background)
                        .border_color(colors.border)
                        .shadow_xs()
                })
                .when(!selected, |row| {
                    row.hover(|style| style.bg(colors.list_hover))
                })
                .on_click(cx.listener(move |page, _, window, cx| page.select(&target, window, cx)))
                .child(
                    div()
                        .size_8()
                        .flex_none()
                        .rounded_md()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(colors.muted)
                        .child(
                            Icon::new(kind.icon())
                                .small()
                                .text_color(colors.muted_foreground),
                        ),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_sm()
                                .font_medium()
                                .truncate()
                                .child(self.model_label(&model)),
                        )
                        .child(
                            h_flex()
                                .gap_1p5()
                                .text_xs()
                                .text_color(colors.muted_foreground)
                                .child(kind.title())
                                .when(missing, |line| {
                                    line.child(pill(t!("routing.missing"), colors.warning))
                                }),
                        ),
                )
                .child(
                    Switch::new(ElementId::Name(format!("route-enabled-{model}").into()))
                        .small()
                        .checked(enabled)
                        .disabled(is_busy(cx))
                        .on_change(move |enable, _, cx| {
                            let command = if *enable { "enable" } else { "disable" };
                            run_change(
                                Change::new(
                                    t!("routing.toggle_label"),
                                    ["route", command, "--model", toggle_model.as_str()],
                                )
                                .applied(),
                                cx,
                            );
                        }),
                )
        });
        // A route being created shows in the list until it is saved or
        // discarded, so the list never looks empty while the editor is open.
        let new_draft = self
            .draft
            .as_ref()
            .filter(|draft| draft.replace_model.is_none())
            .map(|draft| {
                h_flex()
                    .gap_2p5()
                    .px_2()
                    .py_1p5()
                    .rounded_lg()
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.background)
                    .shadow_xs()
                    .child(
                        div()
                            .size_8()
                            .flex_none()
                            .rounded_md()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(colors.muted)
                            .child(
                                Icon::new(draft.kind.icon())
                                    .small()
                                    .text_color(colors.muted_foreground),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_sm()
                                    .font_medium()
                                    .truncate()
                                    .child(tr!("routing.new_kind", kind = draft.kind.title())),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(colors.muted_foreground)
                                    .child(t!("routing.draft_state")),
                            ),
                    )
            });
        v_flex()
            .w_72()
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(colors.border)
            .bg(colors.muted.opacity(0.35))
            .child(
                h_flex()
                    .pl_4()
                    .pr_2()
                    .pt_2()
                    .pb_1()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(colors.muted_foreground)
                            .child(t!("console.page.routing")),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .rounded_md()
                            .border_1()
                            .border_color(colors.input)
                            .bg(colors.background)
                            .shadow_xs()
                            .overflow_hidden()
                            .child(
                                Button::new("new-route")
                                    .ghost()
                                    .small()
                                    .rounded_none()
                                    .icon(IconName::Plus)
                                    .tooltip(t!("routing.new"))
                                    .disabled(self.models.is_empty())
                                    .dropdown_menu_with_anchor(
                                        Anchor::TopRight,
                                        move |menu, _, _| {
                                            [Kind::Failover, Kind::Fusion, Kind::Schedule]
                                                .into_iter()
                                                .fold(menu, |menu, kind| {
                                                    let view = view.clone();
                                                    menu.item(
                                                        PopupMenuItem::new(kind.title())
                                                            .icon(kind.icon())
                                                            .on_click(move |_, window, cx| {
                                                                view.update(cx, |page, cx| {
                                                                    page.create(kind, window, cx)
                                                                })
                                                                .ok();
                                                            }),
                                                    )
                                                })
                                        },
                                    ),
                            )
                            .child(div().w_px().h_4().bg(colors.border))
                            .child(
                                Button::new("delete-route")
                                    .ghost()
                                    .small()
                                    .rounded_none()
                                    .icon(IconName::Minus)
                                    .tooltip(t!("routing.delete"))
                                    .disabled(!can_remove)
                                    .on_click(cx.listener(|page, _, window, cx| {
                                        page.remove_selected(window, cx)
                                    })),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .id("route-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .pb_2()
                    .gap_1()
                    .children(new_draft)
                    .children(rows),
            )
    }

    fn render_failover(
        &self,
        draft: &Draft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors;
        let simulating = self.simulating;
        let source = draft.text("source");
        let fallbacks = draft.list("fallbacks");
        let view = cx.entity().downgrade();
        let node = |active: bool| {
            h_flex()
                .absolute()
                .h(px(NODE_HEIGHT))
                .pl_3()
                .pr_1()
                .gap_2()
                .rounded_full()
                .bg(colors.popover)
                .border_1()
                .border_color(if active {
                    colors.primary.opacity(0.8)
                } else {
                    colors.border
                })
        };
        let picker_slot =
            |picker: AnyElement| div().flex_1().min_w_0().overflow_hidden().child(picker);
        let fallback_nodes: Vec<AnyElement> = fallbacks
            .iter()
            .enumerate()
            .map(|(index, model)| {
                let remove_view = view.clone();
                let up_view = view.clone();
                let picker = self.model_picker(
                    ElementId::Name(format!("fallback-{index}").into()),
                    Some(model.clone()),
                    window,
                    cx,
                    move |draft, picked| {
                        let mut list = draft.list("fallbacks");
                        if let Some(slot) = list.get_mut(index) {
                            *slot = picked;
                        }
                        draft.set("fallbacks", json!(list));
                    },
                );
                let trying = simulating && index == 0;
                node(trying)
                    .left(px(CHILD_LEFT))
                    .top(px(child_top(index + 1)))
                    .w(px(CHILD_WIDTH))
                    .child(
                        div()
                            .size_5()
                            .flex_none()
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .font_medium()
                            .when(trying, |badge| {
                                badge
                                    .bg(colors.primary)
                                    .text_color(colors.primary_foreground)
                            })
                            .when(!trying, |badge| {
                                badge.bg(colors.muted).text_color(colors.muted_foreground)
                            })
                            .child(format!("{}", index + 1)),
                    )
                    .child(picker_slot(picker))
                    .child(
                        Button::new(ElementId::Name(format!("fallback-up-{index}").into()))
                            .ghost()
                            .xsmall()
                            .icon(IconName::ChevronUp)
                            .disabled(index == 0)
                            .on_click(move |_, _, cx| {
                                Self::update_draft(&up_view, cx, |draft| {
                                    let mut list = draft.list("fallbacks");
                                    list.swap(index, index - 1);
                                    draft.set("fallbacks", json!(list));
                                });
                            }),
                    )
                    .child(
                        Button::new(ElementId::Name(format!("fallback-remove-{index}").into()))
                            .ghost()
                            .xsmall()
                            .icon(IconName::X)
                            .on_click(move |_, _, cx| {
                                Self::update_draft(&remove_view, cx, |draft| {
                                    let mut list = draft.list("fallbacks");
                                    list.remove(index);
                                    draft.set("fallbacks", json!(list));
                                });
                            }),
                    )
                    .into_any_element()
            })
            .collect();
        let can_add = fallbacks.len() < MAX_FALLBACKS;
        // The success leaf, each fallback, and the add leaf.
        let children = 1 + fallbacks.len() + usize::from(can_add);
        let root = node(!simulating)
            .left(px(MAP_PAD))
            .top(px(root_top(children)))
            .w(px(ROOT_WIDTH))
            .child(
                Icon::new(IconName::LogIn)
                    .size_4()
                    .text_color(if simulating {
                        colors.warning
                    } else {
                        colors.primary
                    }),
            )
            .child(picker_slot(self.model_picker(
                "failover-source",
                source,
                window,
                cx,
                |draft, picked| draft.set("source", json!(picked)),
            )));
        let success = node(false)
            .left(px(CHILD_LEFT))
            .top(px(child_top(0)))
            .pr_3()
            .child(
                Icon::new(IconName::CircleCheck)
                    .size_4()
                    .text_color(colors.success),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(colors.muted_foreground)
                    .child(t!("routing.finish")),
            );
        let first = self
            .models
            .first()
            .map(|model| model.id.clone())
            .unwrap_or_default();
        let add_view = view.clone();
        let add = can_add.then(|| {
            h_flex()
                .id("add-fallback")
                .absolute()
                .left(px(CHILD_LEFT))
                .top(px(child_top(children - 1)))
                .h(px(NODE_HEIGHT))
                .px_3()
                .gap_2()
                .rounded_full()
                .border_1()
                .border_dashed()
                .border_color(colors.border)
                .text_sm()
                .text_color(colors.muted_foreground)
                .cursor_pointer()
                .hover(|style| style.bg(colors.list_hover).text_color(colors.foreground))
                .child(Icon::new(IconName::Plus).size_4())
                .child(t!("routing.add_fallback"))
                .on_click(move |_, _, cx| {
                    let first = first.clone();
                    Self::update_draft(&add_view, cx, move |draft| {
                        let mut list = draft.list("fallbacks");
                        list.push(first);
                        draft.set("fallbacks", json!(list));
                    });
                })
        });
        let stage = div()
            .relative()
            .w_full()
            .min_w(px(MAP_MIN_WIDTH))
            .h(px(map_height(children)))
            .child(map_branches(children, can_add, simulating, cx))
            .child(root)
            .child(success)
            .children(fallback_nodes)
            .children(add);
        div()
            .id("failover-map")
            .w_full()
            .overflow_x_scroll()
            .child(stage)
            .into_any_element()
    }

    fn number_picker(
        &self,
        id: impl Into<ElementId>,
        current: u64,
        options: Vec<(u64, SharedString)>,
        cx: &mut Context<Self>,
        on_pick: impl Fn(&mut Draft, u64) + 'static,
    ) -> AnyElement {
        let label = options
            .iter()
            .find(|(value, _)| *value == current)
            .map_or_else(
                || SharedString::from(current.to_string()),
                |(_, label)| label.clone(),
            );
        let view = cx.entity().downgrade();
        let on_pick = std::rc::Rc::new(on_pick);
        Button::new(id)
            .secondary()
            .small()
            .label(label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                options.iter().fold(
                    menu.scrollable(true).max_h(gpui_kit::px(320.0)),
                    |menu, (value, label)| {
                        let view = view.clone();
                        let on_pick = on_pick.clone();
                        let value = *value;
                        menu.item(
                            PopupMenuItem::new(label.clone())
                                .checked(value == current)
                                .on_click(move |_, _, cx| {
                                    let on_pick = on_pick.clone();
                                    Self::update_draft(&view, cx, move |draft| {
                                        on_pick(draft, value)
                                    });
                                }),
                        )
                    },
                )
            })
            .into_any_element()
    }

    fn render_fusion(
        &self,
        draft: &Draft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors;
        let panels = draft.list("panel_models");
        let view = cx.entity().downgrade();
        let members = self.models.iter().map(|model| {
            let checked = panels.contains(&model.id);
            let id = model.id.clone();
            let view = view.clone();
            div().min_w_0().px_3().py_2().child(
                Checkbox::new(ElementId::Name(format!("panel-{}", model.id).into()))
                    .label(self.model_label(&model.id))
                    .checked(checked)
                    .on_change(move |checked, _, cx| {
                        let id = id.clone();
                        let checked = *checked;
                        Self::update_draft(&view, cx, move |draft| {
                            let mut list = draft.list("panel_models");
                            list.retain(|model| model != &id);
                            if checked {
                                list.push(id);
                            }
                            draft.set("panel_models", json!(list));
                        });
                    }),
            )
        });
        let minimum = draft
            .payload
            .get("min_successful")
            .and_then(Value::as_u64)
            .unwrap_or(1);
        let timeout = draft
            .payload
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(120_000)
            / 1_000;
        let intermediate = draft
            .payload
            .get("show_intermediate_results")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let toggle_view = view.clone();
        let settings = v_flex()
            .child(setting_row(
                t!("routing.min_successful"),
                None,
                self.number_picker(
                    "fusion-minimum",
                    minimum,
                    (1..=panels.len().max(1) as u64)
                        .map(|value| (value, SharedString::from(value.to_string())))
                        .collect(),
                    cx,
                    |draft, value| draft.set("min_successful", json!(value)),
                ),
                cx,
            ))
            .child(row_divider(cx))
            .child(setting_row(
                t!("routing.timeout"),
                None,
                self.number_picker(
                    "fusion-timeout",
                    timeout,
                    TIMEOUTS_SECONDS
                        .iter()
                        .map(|seconds| (*seconds, tr!("time.seconds", count = seconds)))
                        .collect(),
                    cx,
                    |draft, seconds| draft.set("timeout_ms", json!(seconds * 1_000)),
                ),
                cx,
            ))
            .child(row_divider(cx))
            .child(setting_row(
                t!("routing.intermediate"),
                Some(tr!("routing.intermediate_detail")),
                Switch::new("fusion-intermediate")
                    .checked(intermediate)
                    .on_change(move |value, _, cx| {
                        let value = *value;
                        Self::update_draft(&toggle_view, cx, move |draft| {
                            draft.set("show_intermediate_results", json!(value))
                        });
                    }),
                cx,
            ));
        let outputs = v_flex()
            .flex_1()
            .min_w_0()
            .h(px(FLOW_ROW))
            .relative()
            .child(
                flow_node(cx)
                    .h_full()
                    .flex_col()
                    .items_start()
                    .justify_center()
                    .px_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.muted_foreground)
                            .child(t!("routing.judge")),
                    )
                    .child(self.model_picker(
                        "fusion-judge",
                        draft.text("judge_model"),
                        window,
                        cx,
                        |draft, picked| draft.set("judge_model", json!(picked)),
                    )),
            )
            .child(
                v_flex()
                    .absolute()
                    .top(px(FLOW_ROW))
                    .left_0()
                    .right_0()
                    .child(
                        h_flex().h_6().justify_center().child(
                            Icon::new(IconName::ArrowDown)
                                .small()
                                .text_color(colors.muted_foreground),
                        ),
                    )
                    .child(
                        flow_node(cx)
                            .h(px(FLOW_ROW))
                            .flex_col()
                            .items_start()
                            .justify_center()
                            .px_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(colors.muted_foreground)
                                    .child(t!("routing.final")),
                            )
                            .child(self.model_picker(
                                "fusion-final",
                                draft.text("final_model"),
                                window,
                                cx,
                                |draft, picked| draft.set("final_model", json!(picked)),
                            )),
                    ),
            );
        let count = panels.len().max(1);
        let branches = v_flex()
            .flex_1()
            .min_w_0()
            .when(panels.is_empty(), |list| {
                list.child(
                    h_flex().h(px(FLOW_ROW)).child(
                        flow_node(cx)
                            .w_full()
                            .px_3()
                            .text_color(colors.muted_foreground)
                            .child(t!("routing.none_selected")),
                    ),
                )
            })
            .children(panels.iter().map(|model| {
                h_flex().h(px(FLOW_ROW)).child(
                    flow_node(cx)
                        .w_full()
                        .min_w_0()
                        .px_3()
                        .gap_2()
                        .child(
                            Icon::new(IconName::Layers)
                                .xsmall()
                                .text_color(colors.muted_foreground),
                        )
                        .child(div().min_w_0().truncate().child(self.model_label(model))),
                )
            }));
        let map = h_flex()
            .w_full()
            .min_w(px(495.0))
            .px_4()
            .pt_4()
            .pb(px(88.0))
            .child(
                flow_node(cx)
                    .flex_none()
                    .px_3()
                    .gap_2()
                    .child(
                        Icon::new(IconName::LogIn)
                            .small()
                            .text_color(colors.muted_foreground),
                    )
                    .child(t!("routing.request")),
            )
            .child(flow_branches(count, false, cx))
            .child(branches)
            .child(flow_branches(count, true, cx))
            .child(outputs);
        v_flex()
            .gap_5()
            .child(section(
                t!("routing.panel"),
                Some(tr!("routing.panel_detail", count = panels.len())),
                v_flex()
                    .child(
                        div()
                            .id("fusion-map")
                            .w_full()
                            .overflow_x_scroll()
                            .child(map),
                    )
                    .child(row_divider(cx))
                    .child(
                        div()
                            .px_4()
                            .pt_3()
                            .text_sm()
                            .font_semibold()
                            .child(t!("routing.choose_panel_models")),
                    )
                    .child(div().grid().grid_cols(2).px_1().pb_2().children(members)),
                cx,
            ))
            .child(section(t!("routing.merge"), None, settings, cx))
            .into_any_element()
    }

    fn render_schedule(
        &self,
        draft: &Draft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors;
        let slots: Vec<Value> = draft
            .payload
            .get("time_routes")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let times: Vec<(u64, SharedString)> = (0..=DAY_MINUTES / SLOT_STEP_MINUTES)
            .map(|step| step * SLOT_STEP_MINUTES)
            .map(|minute| (minute, SharedString::from(format_minute(minute))))
            .collect();
        let view = cx.entity().downgrade();
        let rows = slots.iter().enumerate().map(|(index, slot)| {
            let start = slot
                .get("start_minute")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let end = slot
                .get("end_minute")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let model = slot.get("model").and_then(Value::as_str).map(str::to_owned);
            let edit = move |field: &'static str, value: Value| {
                move |draft: &mut Draft| {
                    let mut slots: Vec<Value> = draft
                        .payload
                        .get("time_routes")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    if let Some(slot) = slots.get_mut(index).and_then(Value::as_object_mut) {
                        slot.insert(field.to_owned(), value);
                    }
                    draft.set("time_routes", Value::Array(slots));
                }
            };
            let remove_view = view.clone();
            h_flex().h(px(FLOW_ROW)).child(
                flow_node(cx)
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .px_2()
                    .child(self.number_picker(
                        ElementId::Name(format!("slot-start-{index}").into()),
                        start,
                        times.clone(),
                        cx,
                        move |draft, minute| edit("start_minute", json!(minute))(draft),
                    ))
                    .child(div().text_xs().child("–"))
                    .child(self.number_picker(
                        ElementId::Name(format!("slot-end-{index}").into()),
                        end,
                        times.clone(),
                        cx,
                        move |draft, minute| edit("end_minute", json!(minute))(draft),
                    ))
                    .child(
                        Icon::new(IconName::ArrowRight)
                            .xsmall()
                            .text_color(colors.muted_foreground),
                    )
                    .child(self.model_picker(
                        ElementId::Name(format!("slot-model-{index}").into()),
                        model,
                        window,
                        cx,
                        move |draft, picked| edit("model", json!(picked))(draft),
                    ))
                    .child(div().flex_1().min_w_0())
                    .child(
                        Button::new(ElementId::Name(format!("slot-remove-{index}").into()))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Trash)
                            .on_click(move |_, _, cx| {
                                Self::update_draft(&remove_view, cx, |draft| {
                                    let mut slots: Vec<Value> = draft
                                        .payload
                                        .get("time_routes")
                                        .and_then(Value::as_array)
                                        .cloned()
                                        .unwrap_or_default();
                                    if index < slots.len() {
                                        slots.remove(index);
                                    }
                                    draft.set("time_routes", Value::Array(slots));
                                });
                            }),
                    ),
            )
        });
        let add_view = view.clone();
        let first = self
            .models
            .first()
            .map(|model| model.id.clone())
            .unwrap_or_default();
        let count = slots.len() + 1;
        let branches = v_flex().flex_1().min_w_0().children(rows).child(
            h_flex().h(px(FLOW_ROW)).child(
                flow_node(cx)
                    .w_full()
                    .min_w_0()
                    .px_2()
                    .gap_2()
                    .child(
                        div()
                            .ml_1()
                            .text_sm()
                            .text_color(colors.muted_foreground)
                            .child(t!("routing.otherwise")),
                    )
                    .child(
                        Icon::new(IconName::ArrowRight)
                            .xsmall()
                            .text_color(colors.muted_foreground),
                    )
                    .child(self.model_picker(
                        "schedule-final",
                        draft.text("final_model"),
                        window,
                        cx,
                        |draft, picked| {
                            draft.set("judge_model", json!(picked.clone()));
                            draft.set("final_model", json!(picked));
                        },
                    )),
            ),
        );
        let map = h_flex()
            .w_full()
            .min_w(px(575.0))
            .px_4()
            .pt_3()
            .child(
                flow_node(cx)
                    .flex_none()
                    .px_3()
                    .gap_2()
                    .child(
                        Icon::new(IconName::Clock)
                            .small()
                            .text_color(colors.muted_foreground),
                    )
                    .child(t!("routing.schedule_clock")),
            )
            .child(flow_branches(count, false, cx))
            .child(branches);
        let schedule = v_flex()
            .child(
                div()
                    .id("schedule-map")
                    .w_full()
                    .overflow_x_scroll()
                    .child(map),
            )
            .child(
                div().px_4().py_2().child(
                    Button::new("add-slot")
                        .ghost()
                        .small()
                        .icon(IconName::Plus)
                        .label(t!("routing.add_slot"))
                        .on_click(move |_, _, cx| {
                            let first = first.clone();
                            Self::update_draft(&add_view, cx, move |draft| {
                                let mut slots: Vec<Value> = draft
                                    .payload
                                    .get("time_routes")
                                    .and_then(Value::as_array)
                                    .cloned()
                                    .unwrap_or_default();
                                slots.push(
                                json!({ "start_minute": 0, "end_minute": 6 * 60, "model": first }),
                            );
                                draft.set("time_routes", Value::Array(slots));
                            });
                        }),
                ),
            );
        v_flex()
            .gap_5()
            .child(section(
                t!("routing.slots"),
                Some(tr!("routing.slots_detail")),
                schedule,
                cx,
            ))
            .into_any_element()
    }

    fn render_editor(
        &self,
        draft: &Draft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors;
        let kind = draft.kind;
        let title = draft.replace_model.as_deref().map_or_else(
            || tr!("routing.new_kind", kind = kind.title()),
            |model| self.model_label(model),
        );
        let body = match kind {
            Kind::Failover => self.render_failover(draft, window, cx),
            Kind::Fusion => self.render_fusion(draft, window, cx),
            Kind::Schedule => self.render_schedule(draft, window, cx),
        };
        v_flex()
            .w_full()
            .max_w(gpui_kit::rems(48.0))
            .gap_5()
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        div()
                            .size_10()
                            .rounded_lg()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(colors.muted)
                            .border_1()
                            .border_color(colors.border)
                            .child(Icon::new(kind.icon()).text_color(colors.muted_foreground)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_lg().font_semibold().truncate().child(title)),
                    )
                    .when(kind == Kind::Failover, |header| {
                        let simulating = self.simulating;
                        header.child(
                            Button::new("simulate-failover")
                                .ghost()
                                .small()
                                .icon(if simulating {
                                    IconName::Pause
                                } else {
                                    IconName::Play
                                })
                                .label(if simulating {
                                    t!("routing.stop_simulation")
                                } else {
                                    t!("routing.simulate_failure")
                                })
                                .on_click(cx.listener(|page, _, _, cx| {
                                    page.simulating = !page.simulating;
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .when(
                kind != Kind::Failover || draft.payload.contains_key("id"),
                |editor| {
                    editor.child(
                        h_flex()
                            .gap_3()
                            .child(div().text_sm().w_24().child(t!("routing.name")))
                            .child(div().w_72().child(Input::new(&self.name).small())),
                    )
                },
            )
            .child(body)
    }
}

impl Render for RoutingPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let Some(routes) = self.routes.clone() else {
            return match self.error.clone() {
                Some(error) => placeholder(
                    IconName::TriangleAlert,
                    t!("routing.load_failed"),
                    Some(error),
                    None,
                    cx,
                )
                .into_any_element(),
                None => placeholder(
                    IconName::LoaderCircle,
                    t!("routing.loading"),
                    None,
                    None,
                    cx,
                )
                .into_any_element(),
            };
        };
        let editor = match &self.draft {
            Some(draft) => self.render_editor(draft, window, cx).into_any_element(),
            None if self.models.is_empty() => placeholder(
                IconName::Route,
                t!("routing.no_models"),
                Some(tr!("routing.no_models_detail")),
                None,
                cx,
            )
            .into_any_element(),
            None => {
                // The start page: one card per route type, each showing how
                // the route sends a request and creating a draft on click.
                let cards = [
                    (
                        Kind::Fusion,
                        tr!("routing.kind.fusion_detail"),
                        [
                            tr!("routing.flow.panels"),
                            tr!("routing.flow.judge"),
                            tr!("routing.flow.final"),
                        ],
                    ),
                    (
                        Kind::Schedule,
                        tr!("routing.kind.schedule_detail"),
                        [
                            tr!("routing.flow.clock"),
                            tr!("routing.flow.slot_model"),
                            tr!("routing.flow.default_model"),
                        ],
                    ),
                    (
                        Kind::Failover,
                        tr!("routing.kind.failover_detail"),
                        [
                            tr!("routing.flow.source"),
                            tr!("routing.flow.backup_1"),
                            tr!("routing.flow.backup_2"),
                        ],
                    ),
                ]
                .into_iter()
                .map(|(kind, detail, steps)| {
                    let chain = steps.into_iter().enumerate().fold(
                        h_flex().gap_1().flex_wrap(),
                        |chain, (index, step)| {
                            chain
                                .when(index > 0, |chain| {
                                    chain.child(
                                        Icon::new(IconName::ChevronRight)
                                            .xsmall()
                                            .text_color(colors.muted_foreground.opacity(0.6)),
                                    )
                                })
                                .child(
                                    div()
                                        .px_2()
                                        .py_0p5()
                                        .rounded_full()
                                        .border_1()
                                        .border_color(colors.border)
                                        .text_xs()
                                        .text_color(colors.muted_foreground)
                                        .child(step),
                                )
                        },
                    );
                    v_flex()
                        .id(ElementId::Name(format!("route-kind-{}", kind.id()).into()))
                        .gap_3()
                        .p_5()
                        .rounded_xl()
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.background)
                        .cursor_pointer()
                        .hover(|style| {
                            style
                                .border_color(colors.muted_foreground.opacity(0.4))
                                .shadow_sm()
                        })
                        .on_click(
                            cx.listener(move |page, _, window, cx| page.create(kind, window, cx)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .size_10()
                                .rounded_lg()
                                .border_1()
                                .border_color(colors.border)
                                .child(Icon::new(kind.icon()).text_color(colors.foreground)),
                        )
                        .child(
                            v_flex()
                                .gap_0p5()
                                .child(div().text_sm().font_semibold().child(kind.title()))
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(colors.muted_foreground)
                                        .child(detail),
                                ),
                        )
                        .child(chain)
                        .child(
                            h_flex()
                                .pt_3()
                                .gap_1()
                                .text_sm()
                                .font_medium()
                                .child(t!("routing.gallery.create"))
                                .child(Icon::new(IconName::ArrowRight).xsmall()),
                        )
                });
                v_flex()
                    .w_full()
                    .max_w(gpui_kit::rems(52.0))
                    .gap_5()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_lg()
                                    .font_semibold()
                                    .child(t!("routing.gallery.title")),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(colors.muted_foreground)
                                    .child(t!("routing.gallery.subtitle")),
                            ),
                    )
                    .child(div().grid().grid_cols(2).gap_4().children(cards))
                    .into_any_element()
            }
        };
        let dirty = self.draft.as_ref().is_some_and(|draft| draft.dirty);
        h_flex()
            .size_full()
            .child(self.render_list(&routes, cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(
                        div()
                            .id("route-editor")
                            .size_full()
                            .overflow_y_scroll()
                            .px_6()
                            .pt_5()
                            .pb_24()
                            .child(editor),
                    )
                    .when(dirty, |pane| {
                        pane.child(
                            h_flex()
                                .absolute()
                                .bottom_6()
                                .left_0()
                                .right_0()
                                .justify_center()
                                .child(
                                    h_flex()
                                        .gap_3()
                                        .pl_4()
                                        .pr_2()
                                        .py_2()
                                        .rounded_xl()
                                        .bg(colors.popover)
                                        .border_1()
                                        .border_color(colors.border)
                                        .shadow_lg()
                                        .child(div().text_sm().child(t!("routing.unsaved")))
                                        .child(
                                            Button::new("discard-route")
                                                .ghost()
                                                .small()
                                                .label(t!("common.discard"))
                                                .on_click(cx.listener(|page, _, _, cx| {
                                                    page.draft = None;
                                                    page.selected = None;
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            Button::new("save-route")
                                                .primary()
                                                .small()
                                                .label(t!("routing.save"))
                                                .loading(is_busy(cx))
                                                .disabled(is_busy(cx))
                                                .on_click(
                                                    cx.listener(|page, _, _, cx| page.save(cx)),
                                                ),
                                        ),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }
}

// Failover mind map: the entry model is the root on the left; its branches
// fan out to the success leaf, each fallback in order, and the add leaf.
// Every node has a fixed height, so the curves share the layout's geometry.
const MAP_PAD: f32 = 4.0;
const ROOT_WIDTH: f32 = 232.0;
const BRANCH_GAP: f32 = 104.0;
const CHILD_LEFT: f32 = MAP_PAD + ROOT_WIDTH + BRANCH_GAP;
const CHILD_WIDTH: f32 = 288.0;
const NODE_HEIGHT: f32 = 40.0;
const CHILD_GAP: f32 = 18.0;
const MAP_TOP: f32 = 24.0;
const MAP_MIN_WIDTH: f32 = CHILD_LEFT + CHILD_WIDTH + MAP_PAD;
/// One request travels a branch per cycle.
const FLOW_CYCLE: Duration = Duration::from_millis(2400);
/// Beads drawn behind the request, fading out.
const WAKE_BEADS: usize = 6;

fn child_top(index: usize) -> f32 {
    MAP_TOP + index as f32 * (NODE_HEIGHT + CHILD_GAP)
}

fn map_height(children: usize) -> f32 {
    child_top(children) - CHILD_GAP + MAP_PAD
}

/// The root sits level with the middle of its children.
fn root_top(children: usize) -> f32 {
    (child_top(0) + child_top(children - 1)) / 2.0
}

/// A cubic Bezier branch between two node edges.
#[derive(Clone, Copy)]
struct Curve {
    from: Point<Pixels>,
    control_a: Point<Pixels>,
    control_b: Point<Pixels>,
    to: Point<Pixels>,
}

const FLOW_ROW: f32 = 52.0;

fn flow_node(cx: &App) -> gpui_kit::Div {
    let colors = cx.theme().colors;
    h_flex()
        .min_h_8()
        .rounded_lg()
        .border_1()
        .border_color(colors.border)
        .bg(colors.background)
        .shadow_xs()
        .text_sm()
}

fn flow_branches(children: usize, reverse: bool, cx: &App) -> impl IntoElement {
    let color = cx.theme().colors.muted_foreground.opacity(0.45);
    div()
        .w_8()
        .flex_none()
        .h(px(FLOW_ROW * children as f32))
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let left = bounds.left();
                    let right = bounds.right();
                    let center = bounds.center().y;
                    for index in 0..children {
                        let row = bounds.top() + px((index as f32 + 0.5) * FLOW_ROW);
                        let (from, to) = if reverse {
                            (point(left, row), point(right, center))
                        } else {
                            (point(left, center), point(right, row))
                        };
                        Curve::level(from, to).stroke(1.5, false, window, color);
                    }
                },
            )
            .size_full(),
        )
}

impl Curve {
    /// A horizontal S-curve, leaving and entering its nodes level.
    fn level(from: Point<Pixels>, to: Point<Pixels>) -> Self {
        let pull = (to.x - from.x) * 0.55;
        Self {
            from,
            control_a: point(from.x + pull, from.y),
            control_b: point(to.x - pull, to.y),
            to,
        }
    }

    fn at(&self, t: f32) -> Point<Pixels> {
        let inverse = 1.0 - t;
        let weights = [
            inverse * inverse * inverse,
            3.0 * inverse * inverse * t,
            3.0 * inverse * t * t,
            t * t * t,
        ];
        let points = [self.from, self.control_a, self.control_b, self.to];
        let mix = |axis: fn(&Point<Pixels>) -> f32| {
            px(points
                .iter()
                .zip(weights)
                .map(|(point, weight)| axis(point) * weight)
                .sum())
        };
        point(
            mix(|point| f32::from(point.x)),
            mix(|point| f32::from(point.y)),
        )
    }

    fn stroke(&self, width: f32, dashed: bool, window: &mut Window, color: Hsla) {
        let mut path = PathBuilder::stroke(px(width));
        if dashed {
            path = path.dash_array(&[px(3.0), px(4.0)]);
        }
        path.move_to(self.from);
        path.cubic_bezier_to(self.to, self.control_a, self.control_b);
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
    }
}

/// The branches and the travelling request. At rest the request takes the
/// success branch, which breathes; a simulated failure sends it to the
/// first fallback instead. Each journey eases on the product move curve.
fn map_branches(children: usize, can_add: bool, simulating: bool, cx: &App) -> impl IntoElement {
    let colors = cx.theme().colors;
    let idle = colors.muted_foreground.opacity(0.35);
    let active = colors.primary;
    let render_branches = move |layer: gpui_kit::Div, cycle: f32| {
        let ease = theme::curve(theme::MOVE_CURVE);
        let breath = 0.5 - 0.5 * (cycle * std::f32::consts::TAU).cos();
        layer.child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let at =
                        |x: f32, y: f32| point(bounds.origin.x + px(x), bounds.origin.y + px(y));
                    let root = at(MAP_PAD + ROOT_WIDTH, root_top(children) + NODE_HEIGHT / 2.0);
                    let branches: Vec<Curve> = (0..children)
                        .map(|index| {
                            Curve::level(root, at(CHILD_LEFT, child_top(index) + NODE_HEIGHT / 2.0))
                        })
                        .collect();
                    // The success leaf is index 0; the first fallback is 1.
                    let lit = usize::from(simulating && children > 1 + usize::from(can_add));
                    for (index, branch) in branches.iter().enumerate() {
                        if index == lit {
                            continue;
                        }
                        let dashed = can_add && index == children - 1;
                        branch.stroke(1.25, dashed, window, idle);
                    }
                    let branch = branches[lit];
                    branch.stroke(6.0, false, window, active.opacity(0.06 + 0.1 * breath));
                    branch.stroke(1.5, false, window, active.opacity(0.6 + 0.3 * breath));
                    for bead in 0..WAKE_BEADS {
                        let progress = (cycle - bead as f32 * 0.02) / 0.7;
                        if !(0.0..1.0).contains(&progress) {
                            continue;
                        }
                        let center = branch.at(ease(progress));
                        let fade = 1.0 - bead as f32 / WAKE_BEADS as f32;
                        let radius = if bead == 0 { 3.5 } else { 2.5 * fade + 0.5 };
                        let color = if bead == 0 {
                            colors.popover_foreground
                        } else {
                            active.opacity(0.45 * fade)
                        };
                        window.paint_quad(
                            fill(
                                Bounds::centered_at(
                                    center,
                                    size(px(radius * 2.0), px(radius * 2.0)),
                                ),
                                color,
                            )
                            .corner_radii(px(radius)),
                        );
                    }
                    // Root port.
                    window.paint_quad(
                        fill(Bounds::centered_at(root, size(px(6.0), px(6.0))), active)
                            .corner_radii(px(3.0)),
                    );
                },
            )
            .size_full(),
        )
    };
    let layer = div().absolute().inset_0();
    if cx.reduce_motion() {
        render_branches(layer, 1.0).into_any_element()
    } else {
        layer
            .with_animation(
                ElementId::Name(format!("failover-map-{simulating}-{children}").into()),
                Animation::new(FLOW_CYCLE).repeat(),
                render_branches,
            )
            .into_any_element()
    }
}
