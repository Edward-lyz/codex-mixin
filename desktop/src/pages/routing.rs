//! Routing page: failover, fusion and schedule routes. The editor works on
//! the saved route JSON so fields it does not show are preserved; the CLI
//! validates and stores the result.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, WeakEntity, Window, div, prelude::FluentBuilder as _,
};
use rust_i18n::t;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::{is_busy, placeholder, run_change};
use crate::store::{Change, Store, query};
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

    fn description(self) -> SharedString {
        match self {
            Kind::Failover => tr!("routing.kind.failover_detail"),
            Kind::Fusion => tr!("routing.kind.fusion_detail"),
            Kind::Schedule => tr!("routing.kind.schedule_detail"),
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

#[derive(Clone, Debug, Default, Deserialize)]
struct RouteModel {
    id: String,
    #[serde(default)]
    display_name: Option<String>,
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
    models: Vec<RouteModel>,
    error: Option<SharedString>,
    selected: Option<String>,
    draft: Option<Draft>,
    name: Entity<InputState>,
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
            models: Vec::new(),
            error: None,
            selected: None,
            draft: None,
            name,
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
                        page.models = models
                            .models
                            .into_iter()
                            .filter(|model| !model.id.starts_with(VIRTUAL_PREFIX))
                            .collect();
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
        draft.dirty = false;
        run_change(Change::new(t!("routing.save_label"), args).applied(), cx);
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
    /// A dropdown listing every routable model.
    fn model_picker(
        &self,
        id: impl Into<ElementId>,
        current: Option<String>,
        cx: &mut Context<Self>,
        on_pick: impl Fn(&mut Draft, String) + 'static,
    ) -> AnyElement {
        let label = current
            .as_deref()
            .map_or_else(|| tr!("routing.choose_model"), |id| self.model_label(id));
        let options: Vec<(String, SharedString)> = self
            .models
            .iter()
            .map(|model| (model.id.clone(), self.model_label(&model.id)))
            .collect();
        let view = cx.entity().downgrade();
        let on_pick = std::rc::Rc::new(on_pick);
        Button::new(id)
            .outline()
            .small()
            .label(label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                options.iter().fold(
                    menu.scrollable(true).max_h(gpui_kit::px(360.0)),
                    |menu, (model, label)| {
                        let view = view.clone();
                        let on_pick = on_pick.clone();
                        let model = model.clone();
                        menu.item(
                            PopupMenuItem::new(label.clone())
                                .checked(current.as_deref() == Some(model.as_str()))
                                .on_click(move |_, _, cx| {
                                    let model = model.clone();
                                    let on_pick = on_pick.clone();
                                    Self::update_draft(&view, cx, move |draft| {
                                        on_pick(draft, model)
                                    });
                                }),
                        )
                    },
                )
            })
            .into_any_element()
    }

    fn render_list(&self, routes: &[Value], cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
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
                .py_2()
                .rounded_lg()
                .cursor_pointer()
                .when(selected, |row| row.bg(colors.sidebar_accent))
                .when(!selected, |row| {
                    row.hover(|style| style.bg(colors.sidebar_accent.opacity(0.5)))
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
                        .bg(colors.primary.opacity(0.12))
                        .child(Icon::new(kind.icon()).small().text_color(colors.primary)),
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
        v_flex()
            .w_72()
            .flex_none()
            .h_full()
            .bg(colors.sidebar)
            .border_r_1()
            .border_color(colors.sidebar_border)
            .child(
                v_flex()
                    .id("route-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_2()
                    .gap_0p5()
                    .children(rows),
            )
            .child(
                div()
                    .p_2()
                    .border_t_1()
                    .border_color(colors.sidebar_border)
                    .child(
                        Button::new("new-route")
                            .ghost()
                            .small()
                            .w_full()
                            .icon(IconName::Plus)
                            .label(t!("routing.new"))
                            .disabled(self.models.is_empty())
                            .dropdown_menu(move |menu, _, _| {
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
                            }),
                    ),
            )
    }

    fn render_failover(&self, draft: &Draft, cx: &mut Context<Self>) -> AnyElement {
        let source = draft.text("source");
        let fallbacks = draft.list("fallbacks");
        let view = cx.entity().downgrade();
        let rows = fallbacks.iter().enumerate().map(|(index, model)| {
            let remove_view = view.clone();
            let up_view = view.clone();
            let picker = self.model_picker(
                ElementId::Name(format!("fallback-{index}").into()),
                Some(model.clone()),
                cx,
                move |draft, picked| {
                    let mut list = draft.list("fallbacks");
                    if let Some(slot) = list.get_mut(index) {
                        *slot = picked;
                    }
                    draft.set("fallbacks", json!(list));
                },
            );
            h_flex()
                .gap_2()
                .px_4()
                .py_2()
                .child(
                    div()
                        .w_6()
                        .text_xs()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(format!("{}", index + 1)),
                )
                .child(picker)
                .child(div().flex_1())
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
                        .icon(IconName::Trash)
                        .on_click(move |_, _, cx| {
                            Self::update_draft(&remove_view, cx, |draft| {
                                let mut list = draft.list("fallbacks");
                                list.remove(index);
                                draft.set("fallbacks", json!(list));
                            });
                        }),
                )
        });
        let add_view = view.clone();
        let first = self
            .models
            .first()
            .map(|model| model.id.clone())
            .unwrap_or_default();
        let chain = v_flex().children(rows).child(
            div().px_4().py_2().child(
                Button::new("add-fallback")
                    .ghost()
                    .small()
                    .icon(IconName::Plus)
                    .label(t!("routing.add_fallback"))
                    .disabled(fallbacks.len() >= MAX_FALLBACKS)
                    .on_click(move |_, _, cx| {
                        let first = first.clone();
                        Self::update_draft(&add_view, cx, move |draft| {
                            let mut list = draft.list("fallbacks");
                            list.push(first);
                            draft.set("fallbacks", json!(list));
                        });
                    }),
            ),
        );
        v_flex()
            .gap_5()
            .child(section(
                t!("routing.entry"),
                Some(tr!("routing.entry_detail")),
                v_flex().child(setting_row(
                    t!("routing.source"),
                    None,
                    self.model_picker("failover-source", source, cx, |draft, picked| {
                        draft.set("source", json!(picked))
                    }),
                    cx,
                )),
                cx,
            ))
            .child(section(
                t!("routing.fallbacks"),
                Some(tr!("routing.fallbacks_detail")),
                chain,
                cx,
            ))
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
            .outline()
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

    fn render_fusion(&self, draft: &Draft, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors;
        let panels = draft.list("panel_models");
        let view = cx.entity().downgrade();
        let members = self.models.iter().map(|model| {
            let checked = panels.contains(&model.id);
            let id = model.id.clone();
            let view = view.clone();
            div().px_4().py_1p5().child(
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
                t!("routing.judge"),
                Some(tr!("routing.judge_detail")),
                self.model_picker(
                    "fusion-judge",
                    draft.text("judge_model"),
                    cx,
                    |draft, picked| draft.set("judge_model", json!(picked)),
                ),
                cx,
            ))
            .child(row_divider(cx))
            .child(setting_row(
                t!("routing.final"),
                Some(tr!("routing.final_detail")),
                self.model_picker(
                    "fusion-final",
                    draft.text("final_model"),
                    cx,
                    |draft, picked| draft.set("final_model", json!(picked)),
                ),
                cx,
            ))
            .child(row_divider(cx))
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
        v_flex()
            .gap_5()
            .child(section(
                t!("routing.panel"),
                Some(tr!("routing.panel_detail", count = panels.len())),
                v_flex()
                    .py_1()
                    .children(members)
                    .text_color(colors.foreground),
                cx,
            ))
            .child(section(t!("routing.merge"), None, settings, cx))
            .into_any_element()
    }

    fn render_schedule(&self, draft: &Draft, cx: &mut Context<Self>) -> AnyElement {
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
            h_flex()
                .gap_2()
                .px_4()
                .py_2()
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
                .child(Icon::new(IconName::ArrowRight).xsmall())
                .child(self.model_picker(
                    ElementId::Name(format!("slot-model-{index}").into()),
                    model,
                    cx,
                    move |draft, picked| edit("model", json!(picked))(draft),
                ))
                .child(div().flex_1())
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
                )
        });
        let add_view = view.clone();
        let first = self
            .models
            .first()
            .map(|model| model.id.clone())
            .unwrap_or_default();
        let schedule = v_flex().children(rows).child(
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
            .child(section(
                t!("routing.otherwise"),
                None,
                v_flex().child(setting_row(
                    t!("routing.fallback_model"),
                    None,
                    self.model_picker(
                        "schedule-final",
                        draft.text("final_model"),
                        cx,
                        |draft, picked| {
                            draft.set("judge_model", json!(picked.clone()));
                            draft.set("final_model", json!(picked));
                        },
                    ),
                    cx,
                )),
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
        let saved = draft.replace_model.clone();
        let title = saved.as_deref().map_or_else(
            || tr!("routing.new_kind", kind = kind.title()),
            |model| self.model_label(model),
        );
        let body = match kind {
            Kind::Failover => self.render_failover(draft, cx),
            Kind::Fusion => self.render_fusion(draft, cx),
            Kind::Schedule => self.render_schedule(draft, cx),
        };
        let _ = window;
        let delete = saved.map(|model| {
            Button::new("delete-route")
                .ghost()
                .small()
                .icon(IconName::Trash)
                .label(t!("routing.delete"))
                .on_click(move |_, window, cx| {
                    let model = model.clone();
                    window.open_alert_dialog(cx, move |dialog, _, _| {
                        let model = model.clone();
                        dialog
                            .title(tr!("routing.delete_title"))
                            .description(tr!("routing.delete_detail", model = model))
                            .confirm()
                            .ok_text(t!("routing.delete"))
                            .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                            .cancel_text(t!("common.cancel"))
                            .on_ok(move |_, _, cx| {
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
                })
        });
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
                            .bg(colors.primary.opacity(0.12))
                            .child(Icon::new(kind.icon()).text_color(colors.primary)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_lg().font_semibold().truncate().child(title))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(colors.muted_foreground)
                                    .child(kind.description()),
                            ),
                    )
                    .children(delete),
            )
            .when(
                kind != Kind::Failover || draft.payload.contains_key("id"),
                |editor| {
                    editor.child(
                        h_flex()
                            .gap_3()
                            .child(div().text_sm().w_24().child(t!("routing.name")))
                            .child(div().w_72().child(Input::new(&self.name).small()))
                            .when(kind != Kind::Failover, |row| {
                                row.child(
                                    div()
                                        .text_xs()
                                        .text_color(colors.muted_foreground)
                                        .child(t!("routing.name_hint", prefix = VIRTUAL_PREFIX)),
                                )
                            }),
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
            None => placeholder(
                IconName::Route,
                t!("routing.empty"),
                Some(tr!("routing.empty_detail")),
                None,
                cx,
            )
            .into_any_element(),
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
