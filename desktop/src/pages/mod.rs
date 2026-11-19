pub mod agents;
pub mod connection;
pub mod context;
pub mod models;
pub mod providers;
pub mod routing;
pub mod settings;
pub mod usage;

use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, button::Button, v_flex};
use gpui_kit::{
    App, IntoElement, ParentElement as _, SharedString, Styled as _, div,
    prelude::FluentBuilder as _,
};

use crate::store::{Change, Store};
use gpui_kit::assets::IconName;

/// Queues a change; its outcome is reported by the console notification.
pub fn run_change(change: Change, cx: &mut App) {
    Store::global(cx)
        .update(cx, |store, cx| store.run(change, cx))
        .detach();
}

/// Queues a change and returns whether it succeeded, for callers that keep a
/// draft until the CLI confirms it.
pub fn run_change_then(change: Change, cx: &mut App) -> gpui_kit::Task<bool> {
    let task = Store::global(cx).update(cx, |store, cx| store.run(change, cx));
    cx.spawn(async move |_| task.await.is_ok())
}

pub fn is_busy(cx: &App) -> bool {
    Store::global(cx).read(cx).busy().is_some()
}

/// A centered empty, loading or failure state with an optional action.
pub fn placeholder(
    icon: IconName,
    title: impl Into<SharedString>,
    detail: Option<SharedString>,
    action: Option<Button>,
    cx: &App,
) -> impl IntoElement {
    let colors = cx.theme().colors;
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_3()
        .child(
            div()
                .size_12()
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(colors.muted)
                .child(
                    gpui_kit::component::Icon::new(icon)
                        .size_6()
                        .text_color(colors.muted_foreground),
                ),
        )
        .child(
            div()
                .text_base()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .child(title.into()),
        )
        .when_some(detail, |state, detail| {
            state.child(
                div()
                    .max_w_96()
                    .text_sm()
                    .text_center()
                    .text_color(colors.muted_foreground)
                    .child(detail),
            )
        })
        .when_some(action, |state, action| {
            state.child(action.small().primary())
        })
}
