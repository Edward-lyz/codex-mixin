//! Product controls shared by several pages.

use std::rc::Rc;

use gpui_kit::base::motion::transition;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, ElementId, Hsla, InteractiveElement as _, IntoElement, ParentElement,
    RenderOnce, SharedString, StatefulInteractiveElement as _, Styled, Window, div, img,
    prelude::FluentBuilder as _, relative, svg,
};

use crate::theme;

type ChangeHandler = Rc<dyn Fn(&usize, &mut Window, &mut App)>;

/// An equal-width segmented control whose selection pill slides on the
/// product move curve.
#[derive(IntoElement)]
pub struct Segmented {
    id: ElementId,
    items: Vec<SharedString>,
    selected: usize,
    on_change: Option<ChangeHandler>,
}

impl Segmented {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            items: Vec::new(),
            selected: 0,
            on_change: None,
        }
    }

    pub fn items(mut self, items: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.items = items.into_iter().map(Into::into).collect();
        self
    }

    pub fn selected(mut self, selected: usize) -> Self {
        self.selected = selected;
        self
    }

    pub fn on_change(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Segmented {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let count = self.items.len().max(1) as f32;
        let position = transition(
            (self.id.clone(), "indicator"),
            self.selected as f32,
            theme::move_transition(),
            window,
            cx,
        );
        let colors = cx.theme().colors;
        let radius = cx.theme().radius;
        let indicator = div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(relative(position / count))
            .w(relative(1.0 / count))
            .rounded(radius - gpui_kit::px(1.0))
            .bg(colors.tab_active)
            .shadow_xs();
        let segments = self.items.into_iter().enumerate().map(|(index, label)| {
            let selected = index == self.selected;
            let handler = self.on_change.clone();
            div()
                .id(ElementId::NamedChild(
                    std::sync::Arc::new(self.id.clone()),
                    label.clone(),
                ))
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .w_20()
                .h_6()
                .text_xs()
                .font_medium()
                .cursor_pointer()
                .text_color(if selected {
                    colors.tab_active_foreground
                } else {
                    colors.tab_foreground
                })
                .when(!selected, |segment| {
                    segment.hover(|style| style.text_color(colors.foreground))
                })
                .child(label)
                .when_some(handler, |segment, handler| {
                    segment.on_click(move |_, window, cx| handler(&index, window, cx))
                })
        });
        h_flex()
            .id(self.id.clone())
            .p_0p5()
            .rounded(radius)
            .bg(colors.tab_bar_segmented)
            .child(h_flex().relative().child(indicator).children(segments))
    }
}

/// Provider logos drawn without their own fill are tinted like text.
const TEMPLATE_PROVIDER_ICONS: &[&str] = &["openai", "baidu", "custom"];

pub fn provider_icon_path(icon: &str) -> SharedString {
    match icon {
        "agy" => "agents/agy.svg".into(),
        "aws" | "baidu" | "claude" | "custom" | "deepseek" | "kiro" | "openai" | "opencode"
        | "openrouter" => format!("providers/{icon}.svg").into(),
        _ => "providers/custom.svg".into(),
    }
}

/// A rounded tile holding a provider or agent logo.
pub fn logo_tile(path: SharedString, template: bool, large: bool, cx: &App) -> AnyElement {
    let colors = cx.theme().colors;
    let logo: AnyElement = if template {
        svg()
            .path(path)
            .when(large, |logo| logo.size_5())
            .when(!large, |logo| logo.size_4())
            .text_color(colors.foreground)
            .into_any_element()
    } else {
        img(path)
            .when(large, |logo| logo.size_5())
            .when(!large, |logo| logo.size_4())
            .into_any_element()
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .when(large, |tile| tile.size_9().rounded_lg())
        .when(!large, |tile| tile.size_8().rounded_md())
        .bg(colors.background)
        .border_1()
        .border_color(colors.border)
        .child(logo)
        .into_any_element()
}

pub fn provider_logo(icon: &str, large: bool, cx: &App) -> AnyElement {
    logo_tile(
        provider_icon_path(icon),
        TEMPLATE_PROVIDER_ICONS.contains(&icon),
        large,
        cx,
    )
}

/// A small filled status dot.
pub fn status_dot(color: Hsla) -> impl IntoElement {
    div().flex_none().size_2().rounded_full().bg(color)
}

/// A compact capsule label, tinted by its meaning.
pub fn pill(label: impl Into<SharedString>, color: Hsla) -> impl IntoElement {
    div()
        .flex_none()
        .px_2()
        .py_0p5()
        .rounded_full()
        .text_xs()
        .font_medium()
        .text_color(color)
        .bg(color.opacity(0.12))
        .child(label.into())
}

/// A label whose explanation appears on hover instead of as a second line.
fn explained(label: SharedString, detail: Option<SharedString>) -> impl IntoElement {
    // Section titles and row labels can repeat on a page; the explanation
    // tells them apart, so hover state is never shared.
    let id = format!(
        "explained-{label}-{}",
        detail.as_deref().unwrap_or_default()
    );
    div()
        .id(ElementId::Name(id.into()))
        .truncate()
        .child(label)
        .when_some(detail, |label, detail| {
            label.tooltip(move |window, cx| Tooltip::new(detail.clone()).build(window, cx))
        })
}

/// A titled group of settings rows. The title is a quiet label; its
/// explanation shows on hover.
pub fn section(
    title: impl Into<SharedString>,
    detail: Option<SharedString>,
    body: impl IntoElement,
    cx: &App,
) -> impl IntoElement {
    let colors = cx.theme().colors;
    v_flex()
        .gap_1p5()
        .child(
            div()
                .px_1()
                .text_xs()
                .font_medium()
                .text_color(colors.muted_foreground)
                .child(explained(title.into(), detail)),
        )
        .child(
            v_flex()
                .rounded_lg()
                .border_1()
                .border_color(colors.border)
                .bg(colors.group_box)
                .child(body),
        )
}

/// One row inside a [`section`]: the label on the left, its control on the
/// right. The explanation shows when hovering the label.
pub fn setting_row(
    label: impl Into<SharedString>,
    detail: Option<SharedString>,
    control: impl IntoElement,
    _: &App,
) -> impl IntoElement {
    h_flex()
        .gap_4()
        .px_4()
        .min_h_11()
        .py_1p5()
        .justify_between()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_sm()
                .child(explained(label.into(), detail)),
        )
        .child(div().flex_none().child(control))
}

/// A thin divider between [`setting_row`]s.
pub fn row_divider(cx: &App) -> impl IntoElement {
    div().mx_4().h_px().bg(cx.theme().colors.border)
}
