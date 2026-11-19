//! Providers page: the provider list on the left and the selected
//! provider's models or connection settings on the right.

use std::collections::HashMap;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _,
};
use rust_i18n::t;

use super::connection::{ConnectionView, open_add_provider};
use super::models::ModelsView;
use super::placeholder;
use crate::model::Provider;
use crate::store::Store;
use crate::widgets::{Segmented, pill, provider_logo, status_dot};

const MODELS_TAB: usize = 0;
const CONNECTION_TAB: usize = 1;

pub struct ProvidersPage {
    store: Entity<Store>,
    selected: Option<String>,
    tab: usize,
    models: HashMap<String, Entity<ModelsView>>,
    connections: HashMap<String, Entity<ConnectionView>>,
    _subscriptions: Vec<Subscription>,
}

impl ProvidersPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = Store::global(cx);
        let subscription =
            cx.observe_in(&store, window, |page, _, window, cx| page.sync(window, cx));
        let mut page = Self {
            store,
            selected: None,
            tab: MODELS_TAB,
            models: HashMap::new(),
            connections: HashMap::new(),
            _subscriptions: vec![subscription],
        };
        page.sync(window, cx);
        page
    }

    fn providers<'a>(&self, cx: &'a gpui_kit::App) -> &'a [Provider] {
        self.store
            .read(cx)
            .providers
            .value
            .as_ref()
            .map_or(&[], |list| list.providers.as_slice())
    }

    /// Keeps the selection valid and hands fresh snapshots to retained views.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let providers = self.providers(cx).to_vec();
        let still_listed = self
            .selected
            .as_ref()
            .is_some_and(|id| providers.iter().any(|provider| &provider.id == id));
        if !still_listed {
            self.selected = providers.first().map(|provider| provider.id.clone());
        }
        self.models
            .retain(|id, _| providers.iter().any(|provider| &provider.id == id));
        self.connections
            .retain(|id, _| providers.iter().any(|provider| &provider.id == id));
        for provider in providers {
            if let Some(view) = self.models.get(&provider.id) {
                view.update(cx, |view, cx| view.sync(provider.clone(), window, cx));
            }
            if let Some(view) = self.connections.get(&provider.id) {
                view.update(cx, |view, cx| view.sync(provider.clone(), cx));
            }
        }
        self.ensure_views(window, cx);
        cx.notify();
    }

    fn ensure_views(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(provider) = self.selected_provider(cx).cloned() else {
            return;
        };
        if self.tab == MODELS_TAB && !self.models.contains_key(&provider.id) {
            let id = provider.id.clone();
            let view = cx.new(|cx| ModelsView::new(provider, window, cx));
            self.models.insert(id, view);
        } else if self.tab == CONNECTION_TAB && !self.connections.contains_key(&provider.id) {
            let id = provider.id.clone();
            let view = cx.new(|cx| ConnectionView::new(Some(provider), window, cx));
            self.connections.insert(id, view);
        }
    }

    fn selected_provider<'a>(&self, cx: &'a gpui_kit::App) -> Option<&'a Provider> {
        let selected = self.selected.as_ref()?;
        self.providers(cx)
            .iter()
            .find(|provider| &provider.id == selected)
    }

    fn select(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(id);
        self.ensure_views(window, cx);
        cx.notify();
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let rows = self.providers(cx).iter().map(|provider| {
            let selected = self.selected.as_ref() == Some(&provider.id);
            let id = provider.id.clone();
            let (dot, state) = readiness(provider, cx);
            h_flex()
                .id(ElementId::Name(format!("provider-{}", provider.id).into()))
                .gap_2p5()
                .px_2()
                .py_1p5()
                .rounded_lg()
                .cursor_pointer()
                .when(selected, |row| row.bg(colors.list_active))
                .when(!selected, |row| {
                    row.hover(|style| style.bg(colors.list_hover))
                })
                .on_click(
                    cx.listener(move |page, _, window, cx| page.select(id.clone(), window, cx)),
                )
                .child(provider_logo(&provider.icon, false, cx))
                .child(
                    v_flex()
                        .min_w_0()
                        .gap_0p5()
                        .child(
                            div()
                                .text_sm()
                                .font_medium()
                                .truncate()
                                .child(provider.display_name.clone()),
                        )
                        .child(
                            h_flex()
                                .gap_1p5()
                                .text_xs()
                                .text_color(colors.muted_foreground)
                                .child(status_dot(dot))
                                .child(div().truncate().child(t!(
                                    "providers.row_detail",
                                    state = state,
                                    selected = provider.selected_models.len(),
                                    total = provider.cached_models.len()
                                ))),
                        ),
                )
        });
        v_flex()
            .w_64()
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(colors.border)
            .child(
                v_flex()
                    .id("provider-list")
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
                        Button::new("add-provider")
                            .ghost()
                            .small()
                            .w_full()
                            .icon(IconName::Plus)
                            .label(t!("providers.add"))
                            .on_click(|_, window, cx| open_add_provider(window, cx)),
                    ),
            )
    }

    fn render_header(&self, provider: &Provider, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let (state_color, state) = readiness(provider, cx);
        let state_label: SharedString = if provider.is_healthy() {
            t!("providers.ready").into()
        } else {
            state
        };
        h_flex()
            .px_6()
            .pt_2()
            .pb_4()
            .gap_3()
            .child(provider_logo(&provider.icon, true, cx))
            .child(
                v_flex()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_lg()
                                    .font_semibold()
                                    .truncate()
                                    .child(provider.display_name.clone()),
                            )
                            .when(provider.is_official(), |title| {
                                title.child(pill(t!("providers.official"), colors.muted_foreground))
                            })
                            .child(pill(state_label, state_color)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.muted_foreground)
                            .truncate()
                            .child(
                                provider
                                    .base_url
                                    .clone()
                                    .unwrap_or_else(|| provider.id.clone()),
                            ),
                    ),
            )
            .child(div().flex_1())
            .child(
                Segmented::new("provider-tabs")
                    .items([t!("providers.tab.models"), t!("providers.tab.connection")])
                    .selected(self.tab)
                    .on_change(cx.listener(|page, tab: &usize, window, cx| {
                        page.tab = *tab;
                        page.ensure_views(window, cx);
                        cx.notify();
                    })),
            )
    }
}

/// Status color and label for a provider row.
pub fn readiness(provider: &Provider, cx: &gpui_kit::App) -> (gpui_kit::Hsla, SharedString) {
    let colors = cx.theme().colors;
    if !provider.enabled {
        return (
            colors.muted_foreground,
            t!("providers.state.disabled").into(),
        );
    }
    match provider.readiness.as_str() {
        "healthy" => (colors.success, t!("providers.state.healthy").into()),
        "degraded" => (colors.warning, t!("providers.state.degraded").into()),
        other => (colors.warning, other.to_owned().into()),
    }
}

impl Render for ProvidersPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        if store.providers.value.is_none() {
            let content = match store.providers.error.clone() {
                Some(error) => placeholder(
                    IconName::TriangleAlert,
                    t!("providers.load_failed"),
                    Some(error),
                    None,
                    cx,
                )
                .into_any_element(),
                None => placeholder(
                    IconName::LoaderCircle,
                    t!("providers.loading"),
                    None,
                    None,
                    cx,
                )
                .into_any_element(),
            };
            return div().size_full().child(content).into_any_element();
        }
        let Some(provider) = self.selected_provider(cx).cloned() else {
            let add = Button::new("first-provider")
                .label(t!("providers.add"))
                .on_click(|_, window, cx| open_add_provider(window, cx));
            return h_flex()
                .size_full()
                .child(self.render_sidebar(cx))
                .child(placeholder(
                    IconName::Plug,
                    t!("providers.empty_title"),
                    Some(t!("providers.empty_detail").into()),
                    Some(add),
                    cx,
                ))
                .into_any_element();
        };
        let body = if self.tab == MODELS_TAB {
            self.models
                .get(&provider.id)
                .cloned()
                .map(IntoElement::into_any_element)
        } else {
            self.connections
                .get(&provider.id)
                .cloned()
                .map(IntoElement::into_any_element)
        };
        h_flex()
            .size_full()
            .child(self.render_sidebar(cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.render_header(&provider, cx))
                    .child(div().flex_1().min_h_0().children(body)),
            )
            .into_any_element()
    }
}
