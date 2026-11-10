import Cocoa
import SwiftUI

/// Pages of the console window, in sidebar order.
enum ConsolePage: Int, CaseIterable {
    case overview
    case providers
    case agents
    case usage
    case context
    case routing

    var title: String {
        switch self {
        case .overview: return AppLocalization.string("console.page.overview")
        case .providers: return AppLocalization.string("console.page.providers")
        case .agents: return AppLocalization.string("console.page.agents")
        case .usage: return AppLocalization.string("console.page.usage")
        case .context: return AppLocalization.string("console.page.context")
        case .routing: return AppLocalization.string("console.page.routing")
        }
    }

    var symbolName: String {
        switch self {
        case .overview: return "square.grid.2x2"
        case .providers: return "cpu"
        case .agents: return "person.crop.square"
        case .usage: return "chart.bar"
        case .context: return "text.append"
        case .routing: return "arrow.triangle.branch"
        }
    }
}

private let consoleSidebarWidth: CGFloat = 188

/// Hosts the console pages and switches them from a fixed-width native
/// source-list sidebar. Each page keeps its own view controller and state, so
/// switching pages does not discard provider drafts or reload agents, and the
/// context page reloads the CLI settings and statistics when shown.
final class ConsoleContainerController: NSViewController, NSTableViewDataSource, NSTableViewDelegate {
    private var pages: [ConsolePage: NSViewController] = [:]
    private var pageBuilders: [ConsolePage: () -> NSViewController]
    private let onShow: (ConsolePage) async -> Void
    private var pageTasks: [ConsolePage: Task<Void, Never>] = [:]
    private let sidebar = NSTableView()
    private let contentView = NSView()
    private(set) var currentPage: ConsolePage?

    init(pages: [ConsolePage: () -> NSViewController], onShow: @escaping (ConsolePage) async -> Void) {
        self.pageBuilders = pages
        self.onShow = onShow
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override func loadView() {
        view = NSView()
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("page"))
        column.resizingMask = .autoresizingMask
        sidebar.addTableColumn(column)
        sidebar.headerView = nil
        sidebar.style = .sourceList
        sidebar.rowHeight = 28
        sidebar.allowsEmptySelection = false
        sidebar.dataSource = self
        sidebar.delegate = self
        sidebar.setAccessibilityIdentifier("console-pages")
        let scroll = NSScrollView()
        scroll.documentView = sidebar
        scroll.hasVerticalScroller = false
        scroll.drawsBackground = false
        let background = NSVisualEffectView()
        background.material = .sidebar
        background.blendingMode = .behindWindow
        let separator = NSBox()
        separator.boxType = .separator
        for subview in [background, separator, contentView] {
            subview.translatesAutoresizingMaskIntoConstraints = false
            view.addSubview(subview)
        }
        scroll.translatesAutoresizingMaskIntoConstraints = false
        background.addSubview(scroll)
        NSLayoutConstraint.activate([
            background.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            background.topAnchor.constraint(equalTo: view.topAnchor),
            background.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            background.widthAnchor.constraint(equalToConstant: consoleSidebarWidth),
            scroll.leadingAnchor.constraint(equalTo: background.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: background.trailingAnchor),
            scroll.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor),
            scroll.bottomAnchor.constraint(equalTo: background.bottomAnchor),
            separator.leadingAnchor.constraint(equalTo: background.trailingAnchor),
            separator.widthAnchor.constraint(equalToConstant: 1),
            separator.topAnchor.constraint(equalTo: view.topAnchor),
            separator.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            contentView.leadingAnchor.constraint(equalTo: separator.trailingAnchor),
            contentView.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            contentView.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor),
            contentView.bottomAnchor.constraint(equalTo: view.bottomAnchor),
        ])
    }

    func configureWindow(_ window: NSWindow) {
        // The sidebar replaces the former toolbar tabs; the title shows the
        // current page.
        window.toolbar = nil
        window.titleVisibility = .visible
    }

    func show(_ page: ConsolePage) {
        _ = view
        if pages[page] == nil, let build = pageBuilders.removeValue(forKey: page) {
            let controller = build()
            pages[page] = controller
            addChild(controller)
        }
        guard let controller = pages[page] else { return }
        if currentPage != page {
            contentView.subviews.forEach { $0.removeFromSuperview() }
            let child = controller.view
            child.translatesAutoresizingMaskIntoConstraints = false
            contentView.addSubview(child)
            NSLayoutConstraint.activate([
                child.leadingAnchor.constraint(equalTo: contentView.leadingAnchor),
                child.trailingAnchor.constraint(equalTo: contentView.trailingAnchor),
                child.topAnchor.constraint(equalTo: contentView.topAnchor),
                child.bottomAnchor.constraint(equalTo: contentView.bottomAnchor),
            ])
            currentPage = page
        }
        if sidebar.selectedRow != page.rawValue {
            sidebar.selectRowIndexes(IndexSet(integer: page.rawValue), byExtendingSelection: false)
        }
        view.window?.title = "\(AppLocalization.string("console.windowTitle")) · \(page.title)"
        if pageTasks[page] == nil {
            pageTasks[page] = Task { @MainActor [weak self, onShow] in
                await onShow(page)
                self?.pageTasks.removeValue(forKey: page)
            }
        }
    }

    func releaseContent() {
        pageTasks.values.forEach { $0.cancel() }
        pageTasks.removeAll()
        // SwiftUI/AppKit may retain closed hosting controllers. Clear their
        // roots explicitly so hidden tables and observation graphs can go.
        for controller in pages.values {
            (controller as? NSHostingController<AnyView>)?.rootView = AnyView(EmptyView())
        }
        pageBuilders.removeAll()
    }

    func numberOfRows(in tableView: NSTableView) -> Int {
        ConsolePage.allCases.count
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard let page = ConsolePage(rawValue: row) else { return nil }
        let cell = NSTableCellView()
        let icon = NSImageView(image: NSImage(systemSymbolName: page.symbolName, accessibilityDescription: nil) ?? NSImage())
        let label = NSTextField(labelWithString: page.title)
        label.lineBreakMode = .byTruncatingMiddle
        for subview in [icon, label] {
            subview.translatesAutoresizingMaskIntoConstraints = false
            cell.addSubview(subview)
        }
        cell.imageView = icon
        cell.textField = label
        NSLayoutConstraint.activate([
            icon.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 2),
            icon.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            icon.widthAnchor.constraint(equalToConstant: 18),
            label.leadingAnchor.constraint(equalTo: icon.trailingAnchor, constant: 6),
            label.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -4),
            label.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
        ])
        return cell
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        guard let page = ConsolePage(rawValue: sidebar.selectedRow), page != currentPage else { return }
        show(page)
    }
}
