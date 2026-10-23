import Cocoa
import SwiftUI

/// Pages of the console window, in toolbar order.
enum ConsolePage: Int, CaseIterable {
    case providers
    case agents
    case usage
    case context

    var title: String {
        switch self {
        case .providers: return AppLocalization.string("console.page.providers")
        case .agents: return AppLocalization.string("console.page.agents")
        case .usage: return AppLocalization.string("console.page.usage")
        case .context: return AppLocalization.string("console.page.context")
        }
    }
}

private let consoleToolbarIdentifier = NSToolbar.Identifier("CodexMixinConsole")
private let consolePagesItem = NSToolbarItem.Identifier("CodexMixinConsolePages")

/// Hosts the console pages and switches them from a segmented control in the
/// window toolbar. Each page keeps its own view controller and state, so
/// switching pages does not discard provider drafts or reload agents, and the
/// context page reloads the CLI settings and statistics when shown.
final class ConsoleContainerController: NSViewController, NSToolbarDelegate {
    private let pages: [ConsolePage: NSViewController]
    private let onShow: (ConsolePage) -> Void
    private let segments = NSSegmentedControl()
    private(set) var currentPage: ConsolePage?

    init(pages: [ConsolePage: NSViewController], onShow: @escaping (ConsolePage) -> Void) {
        self.pages = pages
        self.onShow = onShow
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override func loadView() {
        view = NSView()
        segments.segmentCount = ConsolePage.allCases.count
        segments.trackingMode = .selectOne
        for page in ConsolePage.allCases {
            segments.setLabel(page.title, forSegment: page.rawValue)
        }
        segments.target = self
        segments.action = #selector(segmentChanged)
        segments.setAccessibilityIdentifier("console-pages")
        for page in ConsolePage.allCases {
            if let controller = pages[page] { addChild(controller) }
        }
    }

    func installToolbar(in window: NSWindow) {
        let toolbar = NSToolbar(identifier: consoleToolbarIdentifier)
        toolbar.delegate = self
        toolbar.displayMode = .iconOnly
        toolbar.centeredItemIdentifiers = [consolePagesItem]
        window.toolbar = toolbar
        // The page tabs already say where the user is; a title and subtitle
        // beside them only repeat it.
        window.titleVisibility = .hidden
    }

    func show(_ page: ConsolePage) {
        guard let controller = pages[page] else { return }
        _ = view
        segments.selectedSegment = page.rawValue
        if currentPage != page {
            view.subviews.forEach { $0.removeFromSuperview() }
            let child = controller.view
            child.translatesAutoresizingMaskIntoConstraints = false
            view.addSubview(child)
            NSLayoutConstraint.activate([
                child.leadingAnchor.constraint(equalTo: view.leadingAnchor),
                child.trailingAnchor.constraint(equalTo: view.trailingAnchor),
                child.topAnchor.constraint(equalTo: view.topAnchor),
                child.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            ])
            currentPage = page
        }
        // The hidden title still names the window for VoiceOver and Mission Control.
        view.window?.title = "\(AppLocalization.string("console.windowTitle")) · \(page.title)"
        onShow(page)
    }

    func releaseContent() {
        // SwiftUI/AppKit may retain closed hosting controllers. Clear their
        // roots explicitly so hidden tables and observation graphs can go.
        for controller in pages.values {
            (controller as? NSHostingController<AnyView>)?.rootView = AnyView(EmptyView())
        }
    }

    @objc private func segmentChanged() {
        guard let page = ConsolePage(rawValue: segments.selectedSegment) else { return }
        show(page)
    }

    func toolbarAllowedItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        [.flexibleSpace, consolePagesItem]
    }

    func toolbarDefaultItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        [.flexibleSpace, consolePagesItem, .flexibleSpace]
    }

    func toolbar(
        _ toolbar: NSToolbar,
        itemForItemIdentifier itemIdentifier: NSToolbarItem.Identifier,
        willBeInsertedIntoToolbar flag: Bool
    ) -> NSToolbarItem? {
        guard itemIdentifier == consolePagesItem else { return nil }
        let item = NSToolbarItem(itemIdentifier: itemIdentifier)
        item.view = segments
        item.label = AppLocalization.string("console.windowTitle")
        return item
    }
}
