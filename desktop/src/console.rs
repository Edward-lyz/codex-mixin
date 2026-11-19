//! The console window: a title bar with page navigation and one retained
//! entity per page, so switching pages keeps drafts and loaded data.

use gpui_kit::assets::IconName;
use gpui_kit::base::motion::transition;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Icon, StyledExt as _, TitleBar, WindowExt as _, h_flex, input, v_flex,
};
use gpui_kit::{
    ActivationPolicy, Animation, AnimationExt as _, AnyElement, AnyWindowHandle, App,
    AppContext as _, Bounds, Context, Entity, Global, InteractiveElement as _, IntoElement,
    KeyBinding, Menu, MenuItem, OsAction, ParentElement as _, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, SystemMenuType, Window,
    WindowBackgroundAppearance, WindowBounds, WindowOptions, div, img, prelude::FluentBuilder as _,
    px, size,
};
use rust_i18n::t;

use crate::pages::{
    agents::AgentsPage, context::ContextPage, providers::ProvidersPage, routing::RoutingPage,
    settings::SettingsPage, usage::UsagePage,
};
use crate::store::{Store, StoreEvent};
use crate::theme;
use crate::widgets::status_dot;

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

const SIDEBAR_WIDTH: f32 = 212.0;
/// Gap between the page sheet and the window edges.
const SHEET_INSET: f32 = 8.0;
/// Space above page content inside the sheet: the rest of the drag strip,
/// which the title bar overlay covers.
const SHEET_HEADROOM: f32 = 30.0;
const NAV_ITEM_HEIGHT: f32 = 32.0;
/// Item height plus the gap between items.
const NAV_STEP: f32 = 34.0;
/// Height of the traffic-light strip; matches GPUI Kit's title bar.
const TITLE_BAR_HEIGHT: Pixels = px(38.0);

impl Page {
    fn icon(self) -> IconName {
        match self {
            Page::Providers => IconName::Server,
            Page::Agents => IconName::Bot,
            Page::Usage => IconName::ChartColumn,
            Page::Context => IconName::Layers,
            Page::Routing => IconName::Route,
            Page::Settings => IconName::Settings,
        }
    }

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

gpui_kit::actions!(
    codex_mixin,
    [
        CloseWindow,
        Minimize,
        Zoom,
        Hide,
        HideOthers,
        ShowAll,
        Quit,
        OpenSettings
    ]
);

/// Installs the app menu, the standard window shortcuts and the Dock
/// presence that follows the console: the app joins the Dock and Command-Tab
/// while the console is open and returns to the menu bar when it closes.
pub fn init(cx: &mut App) {
    // Command on macOS, Control elsewhere.
    let (close, quit, settings) = if cfg!(target_os = "macos") {
        ("cmd-w", "cmd-q", "cmd-,")
    } else {
        ("ctrl-w", "ctrl-q", "ctrl-,")
    };
    cx.bind_keys([
        KeyBinding::new(close, CloseWindow, None),
        KeyBinding::new(quit, Quit, None),
        KeyBinding::new(settings, OpenSettings, None),
    ]);
    if cfg!(target_os = "macos") {
        cx.bind_keys([
            KeyBinding::new("cmd-m", Minimize, None),
            KeyBinding::new("cmd-h", Hide, None),
            KeyBinding::new("alt-cmd-h", HideOthers, None),
        ]);
    }
    cx.on_action(|_: &CloseWindow, cx| {
        if let Some(handle) = cx.active_window() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    });
    cx.on_action(|_: &Minimize, cx| {
        if let Some(handle) = cx.active_window() {
            let _ = handle.update(cx, |_, window, _| window.minimize_window());
        }
    });
    cx.on_action(|_: &Zoom, cx| {
        if let Some(handle) = cx.active_window() {
            let _ = handle.update(cx, |_, window, _| window.zoom_window());
        }
    });
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &OpenSettings, cx| Console::open(Some(Page::Settings), cx));
    cx.on_window_closed(|cx, _| {
        let open = cx
            .try_global::<ConsoleWindow>()
            .is_some_and(|console| cx.windows().contains(&console.0));
        if !open && cx.has_global::<ConsoleWindow>() {
            cx.remove_global::<ConsoleWindow>();
            cx.set_activation_policy(ActivationPolicy::Accessory);
        }
    })
    .detach();
    cx.set_menus([
        Menu::new("Codex Mixin").items([
            MenuItem::action(t!("menu.settings"), OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu(t!("menu.services"), SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action(t!("menu.hide"), Hide),
            MenuItem::action(t!("menu.hide_others"), HideOthers),
            MenuItem::action(t!("menu.show_all"), ShowAll),
            MenuItem::separator(),
            MenuItem::action(t!("menu.quit"), Quit),
        ]),
        Menu::new(t!("menu.edit")).items([
            MenuItem::os_action(t!("menu.undo"), input::Undo, OsAction::Undo),
            MenuItem::os_action(t!("menu.redo"), input::Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action(t!("menu.cut"), input::Cut, OsAction::Cut),
            MenuItem::os_action(t!("menu.copy"), input::Copy, OsAction::Copy),
            MenuItem::os_action(t!("menu.paste"), input::Paste, OsAction::Paste),
            MenuItem::os_action(t!("menu.select_all"), input::SelectAll, OsAction::SelectAll),
        ]),
        Menu::new(t!("menu.window")).items([
            MenuItem::action(t!("menu.minimize"), Minimize),
            MenuItem::action(t!("menu.zoom"), Zoom),
            MenuItem::separator(),
            MenuItem::action(t!("menu.close"), CloseWindow),
        ]),
    ]);
}

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
        // The console is a regular window: it gets a Dock icon, Command-Tab
        // and the app menu with its shortcuts.
        cx.set_activation_policy(ActivationPolicy::Regular);
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
        let bounds = Bounds::centered(None, size(px(1200.0), px(760.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(920.0), px(600.0))),
            app_id: Some("com.edward.codex-mixin".into()),
            // The translucent sidebar shows the desktop through the
            // behind-window blur.
            window_background: if *theme::FROSTED {
                WindowBackgroundAppearance::Blurred
            } else {
                WindowBackgroundAppearance::Opaque
            },
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
            Err(error) => {
                eprintln!("cannot open the console window: {error:#}");
                cx.set_activation_policy(ActivationPolicy::Accessory);
            }
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
        // The sidebar footer shows the gateway state.
        let status = cx.observe(&store, |_, _, cx| cx.notify());
        let providers = cx.new(|cx| ProvidersPage::new(window, cx));
        let mut console = Self {
            page: Page::Providers,
            providers,
            agents: None,
            usage: None,
            context: None,
            routing: None,
            settings: None,
            _subscriptions: vec![subscription, status],
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
        // The Turn view follows the ledger only while it is on screen.
        if let Some(usage) = &self.usage {
            usage.update(cx, |usage, cx| usage.set_active(page == Page::Usage, cx));
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

impl Console {
    fn render_sidebar(
        &self,
        selected: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors;
        let position = transition(
            "console-navigation",
            selected as f32,
            theme::move_transition(),
            window,
            cx,
        );
        let indicator = div()
            .absolute()
            .left_0()
            .right_0()
            .top(px(position * NAV_STEP))
            .h(px(NAV_ITEM_HEIGHT))
            .rounded_lg()
            .bg(colors.sidebar_accent);
        let items = PAGES.iter().enumerate().map(|(index, page)| {
            let active = index == selected;
            let page = *page;
            h_flex()
                .id(SharedString::from(format!("console-nav-{index}")))
                .relative()
                .h(px(NAV_ITEM_HEIGHT))
                .px_2p5()
                .gap_2p5()
                .rounded_lg()
                .text_sm()
                .cursor_pointer()
                .text_color(if active {
                    colors.sidebar_accent_foreground
                } else {
                    colors.sidebar_foreground
                })
                .when(active, |item| item.font_medium())
                .when(!active, |item| {
                    item.hover(|style| style.bg(colors.sidebar_accent.opacity(0.4)))
                })
                .child(Icon::new(page.icon()).size_4().text_color(if active {
                    colors.primary
                } else {
                    colors.muted_foreground
                }))
                .child(page.title())
                .on_click(
                    cx.listener(move |console, _, window, cx| console.select(page, window, cx)),
                )
        });
        let status = Store::global(cx).read(cx).service.value.clone();
        let running = status.as_ref().is_some_and(|status| status.is_running());
        let endpoint = status
            .as_ref()
            .and_then(|status| status.endpoint.clone())
            .map(|endpoint| {
                endpoint
                    .trim_start_matches("http://")
                    .trim_end_matches("/v1")
                    .to_owned()
            });
        let status_label = if status.is_none() {
            t!("console.gateway_unknown")
        } else if running {
            t!("console.gateway_running")
        } else {
            t!("console.gateway_stopped")
        };
        v_flex()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .pt(TITLE_BAR_HEIGHT)
            .px_3()
            .pb_3()
            .child(
                h_flex()
                    .gap_2()
                    .px_1()
                    .pb_4()
                    .child(img("app/app-icon.png").size_5())
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(colors.sidebar_foreground)
                            .child("Codex Mixin"),
                    ),
            )
            .child(
                v_flex()
                    .relative()
                    .gap(px(NAV_STEP - NAV_ITEM_HEIGHT))
                    .child(indicator)
                    .children(items),
            )
            .child(div().flex_1())
            .child(
                v_flex()
                    .gap_0p5()
                    .px_2p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .text_xs()
                            .child(status_dot(if running {
                                colors.success
                            } else {
                                colors.muted_foreground
                            }))
                            .child(status_label),
                    )
                    .when_some(endpoint, |footer, endpoint| {
                        footer.child(
                            div()
                                .pl_4()
                                .text_xs()
                                .text_color(colors.muted_foreground)
                                .truncate()
                                .child(endpoint),
                        )
                    }),
            )
    }
}

impl Render for Console {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let selected = PAGES
            .iter()
            .position(|page| *page == self.page)
            .unwrap_or(0);
        let page_id = SharedString::from(format!("console-page-{selected}"));
        let content = div().size_full().child(self.page_view()).with_animation(
            page_id,
            Animation::new(theme::ENTER_DURATION).with_easing(theme::curve(theme::ENTER_CURVE)),
            |content, progress| content.opacity(progress).mt(px(6.0 * (1.0 - progress))),
        );
        // The window is the frosted sidebar surface; the page sits on it as an
        // opaque inset sheet with a hairline edge.
        div()
            .relative()
            .size_full()
            .bg(colors.sidebar)
            .when(*theme::FROSTED, |surface| {
                surface.bg(colors.sidebar.opacity(theme::FROSTED_TINT))
            })
            .text_color(colors.foreground)
            .child(
                h_flex()
                    .size_full()
                    .child(self.render_sidebar(selected, window, cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .py(px(SHEET_INSET))
                            .pr(px(SHEET_INSET))
                            .child(
                                v_flex()
                                    .size_full()
                                    .rounded_xl()
                                    .overflow_hidden()
                                    .bg(colors.background)
                                    .border_1()
                                    .border_color(colors.border)
                                    .child(div().flex_none().h(px(SHEET_HEADROOM)))
                                    .child(div().flex_1().min_h_0().child(content)),
                            ),
                    ),
            )
            // The title bar spans the window above both columns, so the
            // whole top edge drags the window and keeps the traffic lights.
            .child(
                div().absolute().top_0().left_0().right_0().child(
                    TitleBar::new()
                        .bg(gpui_kit::transparent_black())
                        .border_color(gpui_kit::transparent_black()),
                ),
            )
    }
}
