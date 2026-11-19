//! The console window: a title bar with page navigation and one retained
//! entity per page, so switching pages keeps drafts and loaded data.

use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, TitleBar, WindowExt as _, v_flex};
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, AnyWindowHandle, App, AppContext as _, Bounds,
    Context, Entity, Global, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Window, WindowBounds, WindowOptions, div, prelude::FluentBuilder as _, px, size,
};
use rust_i18n::t;

use crate::pages::{
    agents::AgentsPage, context::ContextPage, providers::ProvidersPage, routing::RoutingPage,
    settings::SettingsPage, usage::UsagePage,
};
use crate::store::{Store, StoreEvent};
use crate::theme;
use crate::widgets::Segmented;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Page {
    Providers,
    Agents,
    Usage,
    Context,
    Routing,
    Settings,
}

const PAGES: [Page; 6] = [
    Page::Providers,
    Page::Agents,
    Page::Usage,
    Page::Context,
    Page::Routing,
    Page::Settings,
];

impl Page {
    fn title(self) -> SharedString {
        match self {
            Page::Providers => t!("console.page.providers"),
            Page::Agents => t!("console.page.agents"),
            Page::Usage => t!("console.page.usage"),
            Page::Context => t!("console.page.context"),
            Page::Routing => t!("console.page.routing"),
            Page::Settings => t!("console.page.settings"),
        }
        .into()
    }
}

struct ConsoleWindow(AnyWindowHandle);

impl Global for ConsoleWindow {}

pub struct Console {
    page: Page,
    providers: Entity<ProvidersPage>,
    agents: Option<Entity<AgentsPage>>,
    usage: Option<Entity<UsagePage>>,
    context: Option<Entity<ContextPage>>,
    routing: Option<Entity<RoutingPage>>,
    settings: Option<Entity<SettingsPage>>,
    _subscriptions: Vec<Subscription>,
}

impl Console {
    /// Shows the console, reusing the open window.
    pub fn open(page: Option<Page>, cx: &mut App) {
        if let Some(handle) = cx.try_global::<ConsoleWindow>().map(|window| window.0)
            && cx.windows().contains(&handle)
        {
            let _ = handle.update(cx, |root, window, cx| {
                window.activate_window();
                if let (Some(page), Ok(root)) = (page, root.downcast::<gpui_kit::base::Root>())
                    && let Ok(console) = root.read(cx).view().clone().downcast::<Console>()
                {
                    console.update(cx, |console, cx| console.select(page, window, cx));
                }
            });
            cx.activate(true);
            return;
        }
        let bounds = Bounds::centered(None, size(px(1120.0), px(740.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(920.0), px(600.0))),
            app_id: Some("com.edward.codex-mixin".into()),
            ..TitleBar::window_options()
        };
        let opened = gpui_kit::open_window(options, cx, |window, cx| {
            theme::follow_system_appearance(window, cx);
            window.set_window_title(&t!("console.window_title"));
            cx.new(|cx| Console::new(page.unwrap_or(Page::Providers), window, cx))
        });
        match opened {
            Ok((handle, _)) => {
                cx.set_global(ConsoleWindow(handle));
                cx.activate(true);
            }
            Err(error) => eprintln!("cannot open the console window: {error:#}"),
        }
    }

    fn new(page: Page, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = Store::global(cx);
        let subscription = cx.subscribe_in(&store, window, |_, _, event, window, cx| {
            let StoreEvent::ChangeFinished { label, result } = event;
            // Only the focused window reports, so the tray panel and the
            // console never show the same outcome twice.
            if !window.is_window_active() {
                return;
            }
            let notification = match result {
                Ok(_) => Notification::success(t!("notice.done", label = label)),
                Err(error) if error.committed == Some(true) => Notification::warning(t!(
                    "notice.saved_with_error",
                    label = label,
                    error = error.message
                )),
                Err(error) => {
                    Notification::error(t!("notice.failed", label = label, error = error.message))
                }
            };
            window.push_notification(notification, cx);
        });
        let providers = cx.new(|cx| ProvidersPage::new(window, cx));
        let mut console = Self {
            page: Page::Providers,
            providers,
            agents: None,
            usage: None,
            context: None,
            routing: None,
            settings: None,
            _subscriptions: vec![subscription],
        };
        console.select(page, window, cx);
        console
    }

    pub fn select(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.page = page;
        match page {
            Page::Providers => {}
            Page::Agents => {
                self.agents
                    .get_or_insert_with(|| cx.new(|cx| AgentsPage::new(window, cx)));
            }
            Page::Usage => {
                self.usage
                    .get_or_insert_with(|| cx.new(|cx| UsagePage::new(window, cx)));
            }
            Page::Context => {
                let page = self
                    .context
                    .get_or_insert_with(|| cx.new(|cx| ContextPage::new(window, cx)));
                // Context statistics move with every request; reload on show.
                page.update(cx, |page, cx| page.reload(cx));
            }
            Page::Routing => {
                self.routing
                    .get_or_insert_with(|| cx.new(|cx| RoutingPage::new(window, cx)));
            }
            Page::Settings => {
                self.settings
                    .get_or_insert_with(|| cx.new(|cx| SettingsPage::new(window, cx)));
            }
        }
        window.set_window_title(&format!(
            "{} · {}",
            t!("console.window_title"),
            page.title()
        ));
        cx.notify();
    }

    fn page_view(&self) -> AnyElement {
        match self.page {
            Page::Providers => self.providers.clone().into_any_element(),
            Page::Agents => self
                .agents
                .clone()
                .map_or_else(empty, IntoElement::into_any_element),
            Page::Usage => self
                .usage
                .clone()
                .map_or_else(empty, IntoElement::into_any_element),
            Page::Context => self
                .context
                .clone()
                .map_or_else(empty, IntoElement::into_any_element),
            Page::Routing => self
                .routing
                .clone()
                .map_or_else(empty, IntoElement::into_any_element),
            Page::Settings => self
                .settings
                .clone()
                .map_or_else(empty, IntoElement::into_any_element),
        }
    }
}

fn empty() -> AnyElement {
    div().into_any_element()
}

impl Render for Console {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let selected = PAGES
            .iter()
            .position(|page| *page == self.page)
            .unwrap_or(0);
        let navigation = Segmented::new("console-pages")
            .large()
            .items(PAGES.iter().map(|page| page.title()))
            .selected(selected)
            .on_change(cx.listener(|console, index: &usize, window, cx| {
                if let Some(page) = PAGES.get(*index) {
                    console.select(*page, window, cx);
                }
            }));
        let page_id = SharedString::from(format!("console-page-{selected}"));
        let content = div().size_full().child(self.page_view()).with_animation(
            page_id,
            Animation::new(theme::ENTER_DURATION).with_easing(theme::curve(theme::ENTER_CURVE)),
            |content, progress| content.opacity(progress).mt(px(6.0 * (1.0 - progress))),
        );
        v_flex()
            .size_full()
            .bg(colors.background)
            .text_color(colors.foreground)
            .child(
                TitleBar::new()
                    .bg(colors.title_bar)
                    .border_color(colors.title_bar_border)
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .justify_center()
                            .when(cfg!(target_os = "macos"), |bar| bar.pr_20())
                            .child(navigation),
                    ),
            )
            .child(div().flex_1().min_h_0().child(content))
    }
}
