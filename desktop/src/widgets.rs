//! Product controls shared by several pages.

use std::rc::Rc;

use gpui_kit::base::motion::transition;
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
    large: bool,
    on_change: Option<ChangeHandler>,
}

impl Segmented {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            items: Vec::new(),
            selected: 0,
            large: false,
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

    /// Window-level navigation uses wider, taller segments.
    pub fn large(mut self) -> Self {
        self.large = true;
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
                .when(self.large, |segment| segment.w_24().h_7().text_sm())
                .when(!self.large, |segment| segment.w_20().h_6().text_xs())
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

/// A titled, bordered group of settings rows.
pub fn section(
    title: impl Into<SharedString>,
    detail: Option<SharedString>,
    body: impl IntoElement,
    cx: &App,
) -> impl IntoElement {
    let colors = cx.theme().colors;
    v_flex()
        .gap_2()
        .child(
            v_flex()
                .gap_0p5()
                .child(div().text_sm().font_semibold().child(title.into()))
                .when_some(detail, |header, detail| {
                    header.child(
                        div()
                            .text_xs()
                            .text_color(colors.muted_foreground)
                            .child(detail),
                    )
                }),
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

/// One row inside a [`section`]: label and detail on the left, control on
/// the right.
pub fn setting_row(
    label: impl Into<SharedString>,
    detail: Option<SharedString>,
    control: impl IntoElement,
    cx: &App,
) -> impl IntoElement {
    let colors = cx.theme().colors;
    h_flex()
        .gap_4()
        .px_4()
        .py_3()
        .justify_between()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(div().text_sm().child(label.into()))
                .when_some(detail, |text, detail| {
                    text.child(
                        div()
                            .text_xs()
                            .text_color(colors.muted_foreground)
                            .child(detail),
                    )
                }),
        )
        .child(div().flex_none().child(control))
}

/// A thin divider between [`setting_row`]s.
pub fn row_divider(cx: &App) -> impl IntoElement {
    div().mx_4().h_px().bg(cx.theme().colors.border)
}
