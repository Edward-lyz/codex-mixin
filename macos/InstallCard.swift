import AppKit
import CryptoKit
import SwiftUI
import UniformTypeIdentifiers

struct CardIdentityV1: Codable, Equatable {
    let installationID: UUID
    let firstRecordedAt: Date
    let seedVersion: UInt8
}

final class CardIdentityStore {
    static let standard = CardIdentityStore()

    private let defaults: UserDefaults
    private let key: String
    private let earliestHistoryDate: () -> Date?

    init(
        defaults: UserDefaults = .standard,
        key: String = "codexMixin.cardIdentity.v1",
        earliestHistoryDate: @escaping () -> Date? = {
            CardIdentityStore.defaultEarliestHistoryDate()
        }
    ) {
        self.defaults = defaults
        self.key = key
        self.earliestHistoryDate = earliestHistoryDate
    }

    @discardableResult
    func current(
        now: Date = Date(),
        makeUUID: () -> UUID = UUID.init
    ) -> CardIdentityV1 {
        let historicalDate = earliestHistoryDate().flatMap { $0 <= now ? $0 : nil }
        if
            let data = defaults.data(forKey: key),
            let identity = try? JSONDecoder().decode(CardIdentityV1.self, from: data)
        {
            let migratedDate = min(
                identity.firstRecordedAt,
                historicalDate ?? identity.firstRecordedAt
            )
            let migratedIdentity = CardIdentityV1(
                installationID: identity.installationID,
                firstRecordedAt: migratedDate,
                seedVersion: 2
            )
            if migratedIdentity != identity {
                save(migratedIdentity)
            }
            return migratedIdentity
        }

        let identity = CardIdentityV1(
            installationID: makeUUID(),
            firstRecordedAt: historicalDate ?? now,
            seedVersion: 2
        )
        save(identity)
        return identity
    }

    func reset() {
        defaults.removeObject(forKey: key)
    }

    private func save(_ identity: CardIdentityV1) {
        if let data = try? JSONEncoder().encode(identity) {
            defaults.set(data, forKey: key)
        }
    }

    private static func defaultEarliestHistoryDate(
        fileManager: FileManager = .default
    ) -> Date? {
        let stateDirectory = fileManager.homeDirectoryForCurrentUser
            .appendingPathComponent(".codex-mixin", isDirectory: true)
        guard fileManager.fileExists(atPath: stateDirectory.path) else {
            return nil
        }

        var candidates = [stateDirectory]
        if let children = try? fileManager.contentsOfDirectory(
            at: stateDirectory,
            includingPropertiesForKeys: [.creationDateKey],
            options: [.skipsHiddenFiles]
        ) {
            candidates.append(contentsOf: children)
        }
        return candidates.compactMap { url in
            try? url.resourceValues(forKeys: [.creationDateKey]).creationDate
        }.min()
    }
}

struct CardWallpaper: Codable, Equatable {
    let fileName: String
    let title: String
    let credit: String
    let sourceURL: String
    let width: Int
    let height: Int
    let sha256: String
}

private struct CardWallpaperManifest: Codable {
    let schemaVersion: Int
    let issue: String
    let sourcePage: String
    let images: [CardWallpaper]
}

enum CardWallpaperCatalog {
    private static let manifest: CardWallpaperManifest? = {
        guard
            let url = directoryURL?.appendingPathComponent("manifest.json"),
            let data = try? Data(contentsOf: url),
            let manifest = try? JSONDecoder().decode(CardWallpaperManifest.self, from: data),
            manifest.schemaVersion == 1
        else {
            return nil
        }
        return manifest
    }()

    static var issue: String {
        manifest?.issue ?? "SPACE ARCHIVE"
    }

    static var sourcePage: String? {
        manifest?.sourcePage
    }

    static var wallpapers: [CardWallpaper] {
        manifest?.images ?? []
    }

    static func image(for wallpaper: CardWallpaper) -> NSImage? {
        guard let url = directoryURL?.appendingPathComponent(wallpaper.fileName) else {
            return nil
        }
        return NSImage(contentsOf: url)
    }

    private static var directoryURL: URL? {
        if let directory = ProcessInfo.processInfo.environment["CODEX_MIXIN_WALLPAPER_ASSET_DIR"] {
            return URL(fileURLWithPath: directory, isDirectory: true)
        }
        return Bundle.main.resourceURL?.appendingPathComponent(
            "Wallpapers",
            isDirectory: true
        )
    }
}

final class CardWallpaperSelectionStore {
    static let standard = CardWallpaperSelectionStore()

    private let defaults: UserDefaults
    private let key: String

    init(
        defaults: UserDefaults = .standard,
        key: String = "codexMixin.cardWallpaper.lastOffset.v1"
    ) {
        self.defaults = defaults
        self.key = key
    }

    func nextOffset(
        count: Int,
        randomIndex: (_ upperBound: Int) -> Int = {
            Int.random(in: 0..<$0)
        }
    ) -> Int {
        guard count > 1 else { return 0 }

        let lastOffset = defaults.object(forKey: key) as? Int
        let candidate: Int
        if let lastOffset, (0..<count).contains(lastOffset) {
            let randomOffset = randomIndex(count - 1)
            candidate = randomOffset >= lastOffset ? randomOffset + 1 : randomOffset
        } else {
            candidate = randomIndex(count)
        }
        defaults.set(candidate, forKey: key)
        return candidate
    }
}

struct InstallCardDesign: Equatable {
    let seed: UInt64
    let wallpaperIndex: Int
    let wallpaper: CardWallpaper?
    let identityCode: String

    init(
        identity: CardIdentityV1,
        wallpaperOffset: Int = 0,
        wallpapers: [CardWallpaper] = CardWallpaperCatalog.wallpapers
    ) {
        let digest = cardIdentityDigest(identity)
        seed = digest.prefix(8).reduce(UInt64.zero) { ($0 << 8) | UInt64($1) }
        wallpaperIndex = cardWallpaperIndex(
            seed: seed,
            count: wallpapers.count,
            offset: wallpaperOffset
        )
        wallpaper = wallpapers.isEmpty ? nil : wallpapers[wallpaperIndex]
        identityCode = digest.prefix(4).map { String(format: "%02X", $0) }.joined()
    }
}

func cardWallpaperIndex(seed: UInt64, count: Int, offset: Int) -> Int {
    guard count > 1 else { return 0 }
    let start = Int(seed % UInt64(count))
    var stride = 1 + Int((seed >> 16) % UInt64(count - 1))
    while greatestCommonDivisor(stride, count) != 1 {
        stride = stride == count - 1 ? 1 : stride + 1
    }
    let normalizedOffset = ((offset % count) + count) % count
    return (start + normalizedOffset * stride) % count
}

private func greatestCommonDivisor(_ lhs: Int, _ rhs: Int) -> Int {
    var a = lhs
    var b = rhs
    while b != 0 {
        (a, b) = (b, a % b)
    }
    return a
}

func cardIdentityDigest(_ identity: CardIdentityV1) -> [UInt8] {
    let recordedDay = Int64(floor(identity.firstRecordedAt.timeIntervalSince1970 / 86_400))
    let payload = [
        "codex-mixin-card",
        identity.installationID.uuidString.lowercased(),
        String(recordedDay),
        String(identity.seedVersion),
    ].joined(separator: "|")
    return Array(SHA256.hash(data: Data(payload.utf8)))
}

func cardDayCount(
    identity: CardIdentityV1,
    now: Date = Date(),
    calendar: Calendar = .current
) -> Int {
    let start = calendar.startOfDay(for: identity.firstRecordedAt)
    let end = calendar.startOfDay(for: max(now, identity.firstRecordedAt))
    return max(1, (calendar.dateComponents([.day], from: start, to: end).day ?? 0) + 1)
}

func cardRecordedMonth(identity: CardIdentityV1, locale: Locale = .current) -> String {
    let formatter = DateFormatter()
    formatter.locale = locale
    formatter.setLocalizedDateFormatFromTemplate("yyyyMMM")
    return formatter.string(from: identity.firstRecordedAt)
}

private func cardWallpaperIssueLabel(_ issue: String) -> String {
    let parts = issue.split(separator: "-")
    guard
        parts.count == 2,
        let year = Int(parts[0]),
        let month = Int(parts[1]),
        (1...12).contains(month)
    else {
        return issue.uppercased()
    }
    let formatter = DateFormatter()
    formatter.locale = Locale(identifier: "en_US_POSIX")
    let symbols = formatter.shortMonthSymbols ?? []
    let monthName = month <= symbols.count ? symbols[month - 1] : String(month)
    return "\(monthName.uppercased()) \(year)"
}

struct InstallCardSurface: View {
    let identity: CardIdentityV1
    let wallpaperOffset: Int
    let elapsed: TimeInterval
    let pointer: CGPoint
    let drag: CGSize
    let revealed: Bool
    let now: Date

    private var design: InstallCardDesign {
        InstallCardDesign(identity: identity, wallpaperOffset: wallpaperOffset)
    }

    var body: some View {
        GeometryReader { geometry in
            let size = geometry.size
            ZStack {
                wallpaper(size: size)
                readabilityOverlay
                    .frame(width: size.width, height: size.height)
                cardCopy(size: size)
                    .frame(width: size.width, height: size.height)
            }
            .frame(width: size.width, height: size.height)
            .clipped()
        }
        .aspectRatio(1.6, contentMode: .fit)
        .clipShape(RoundedRectangle(cornerRadius: 20, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 20, style: .continuous)
                .stroke(Color.white.opacity(0.22), lineWidth: 1)
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel(
            AppLocalization.string("installCard.codexMixinCardDayWithMixin", cardDayCount(identity: identity, now: now))
        )
    }

    @ViewBuilder
    private func wallpaper(size: CGSize) -> some View {
        if
            let selected = design.wallpaper,
            let image = CardWallpaperCatalog.image(for: selected)
        {
            let parallaxX = (pointer.x - 0.5) * size.width * 0.024 + drag.width * 0.035
            let parallaxY = (pointer.y - 0.5) * size.height * 0.03 + drag.height * 0.035
            Image(nsImage: image)
                .resizable()
                .scaledToFill()
                .frame(width: size.width * 1.07, height: size.height * 1.07)
                .offset(x: parallaxX, y: parallaxY)
                .scaleEffect(revealed ? 1.025 : 1)
                .saturation(revealed ? 1.08 : 1)
                .id(selected.fileName)
                .transition(.opacity)
                .animation(.spring(response: 0.5, dampingFraction: 0.82), value: revealed)
                .animation(.easeInOut(duration: 0.9), value: selected.fileName)
        } else {
            LinearGradient(
                colors: [Color(red: 0.02, green: 0.05, blue: 0.1), .black],
                startPoint: .topLeading,
                endPoint: .bottomTrailing
            )
        }
    }

    private var readabilityOverlay: some View {
        ZStack {
            LinearGradient(
                colors: [
                    .black.opacity(0.82),
                    .black.opacity(0.47),
                    .black.opacity(0.06),
                    .clear,
                ],
                startPoint: .leading,
                endPoint: .trailing
            )
            LinearGradient(
                colors: [.clear, .black.opacity(0.52)],
                startPoint: .center,
                endPoint: .bottom
            )
        }
    }

    @ViewBuilder
    private func cardCopy(size: CGSize) -> some View {
        let dayCount = cardDayCount(identity: identity, now: now)
        let edge = max(22, size.width * 0.038)
        let recordedMonth = cardRecordedMonth(
            identity: identity,
            locale: Locale(identifier: "en_US")
        ).uppercased()

        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .firstTextBaseline) {
                HStack(spacing: 8) {
                    Circle()
                        .fill(Color.white)
                        .frame(width: max(4, size.width * 0.006))
                    Text("CODEX MIXIN")
                        .font(.system(
                            size: max(11, size.width * 0.016),
                            weight: .semibold,
                            design: .monospaced
                        ))
                        .tracking(1.6)
                }
                Spacer()
                HStack(spacing: max(6, size.width * 0.008)) {
                    HStack(spacing: max(3, size.width * 0.004)) {
                        ForEach(CardWallpaperCatalog.wallpapers.indices, id: \.self) { index in
                            Circle()
                                .fill(Color.white.opacity(
                                    index == design.wallpaperIndex ? 0.92 : 0.3
                                ))
                                .frame(
                                    width: max(3, size.width * 0.004),
                                    height: max(3, size.width * 0.004)
                                )
                        }
                    }
                    Text(cardWallpaperIssueLabel(CardWallpaperCatalog.issue))
                        .font(.system(
                            size: max(9, size.width * 0.012),
                            weight: .medium,
                            design: .monospaced
                        ))
                        .tracking(1.2)
                        .opacity(0.74)
                }
            }

            Spacer()

            HStack(alignment: .bottom, spacing: 22) {
                VStack(alignment: .leading, spacing: 0) {
                    Text(String(format: "%03d", dayCount))
                        .font(.system(
                            size: max(44, size.width * 0.09),
                            weight: .medium,
                            design: .rounded
                        ))
                        .tracking(-2)
                        .lineLimit(1)
                        .minimumScaleFactor(0.7)

                    Text(revealed
                        ? AppLocalization.string("installCard.stillINORBIT")
                        : AppLocalization.string("installCard.daysWITHMIXIN")
                    )
                    .font(.system(
                        size: max(11, size.width * 0.017),
                        weight: .semibold,
                        design: .monospaced
                    ))
                    .tracking(1.2)
                    .opacity(0.92)

                    Rectangle()
                        .fill(Color.white.opacity(0.5))
                        .frame(width: max(70, size.width * 0.12), height: 1)
                        .padding(.vertical, max(9, size.height * 0.018))

                    Text("SINCE \(recordedMonth)  ·  \(design.identityCode)")
                    .font(.system(
                        size: max(8, size.width * 0.011),
                        weight: .medium,
                        design: .monospaced
                    ))
                    .tracking(0.9)
                    .opacity(0.68)
                }

                Spacer(minLength: 16)

                if let selected = design.wallpaper {
                    VStack(alignment: .trailing, spacing: 4) {
                        Text(selected.title.uppercased())
                            .font(.system(
                                size: max(9, size.width * 0.013),
                                weight: .semibold,
                                design: .monospaced
                            ))
                            .tracking(0.9)
                            .multilineTextAlignment(.trailing)
                        Text("IMAGE  \(selected.credit.uppercased())")
                            .font(.system(
                                size: max(8, size.width * 0.01),
                                weight: .regular,
                                design: .monospaced
                            ))
                            .tracking(0.8)
                            .opacity(0.64)
                    }
                    .frame(maxWidth: size.width * 0.27, alignment: .trailing)
                }
            }
        }
        .foregroundStyle(.white)
        .padding(edge)
        .shadow(color: .black.opacity(0.35), radius: 10, y: 3)
    }
}

struct InstallCardExperienceView: View {
    let identity: CardIdentityV1
    let wallpaperOffset: Int
    let onSave: (_ revealed: Bool, _ wallpaperOffset: Int) -> Void
    let onShare: (_ revealed: Bool, _ wallpaperOffset: Int) -> Void

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var pointer = CGPoint(x: 0.5, y: 0.5)
    @State private var drag = CGSize.zero
    @State private var revealed = false
    @State private var hovering = false

    var body: some View {
        VStack(spacing: 18) {
            GeometryReader { geometry in
                InstallCardSurface(
                    identity: identity,
                    wallpaperOffset: wallpaperOffset,
                    elapsed: 0,
                    pointer: pointer,
                    drag: drag,
                    revealed: revealed,
                    now: Date()
                )
                .scaleEffect(hovering && !reduceMotion ? 1.008 : 1)
                .shadow(
                    color: .black.opacity(hovering ? 0.34 : 0.24),
                    radius: hovering ? 28 : 18,
                    y: hovering ? 16 : 10
                )
                .contentShape(RoundedRectangle(cornerRadius: 20, style: .continuous))
                .onContinuousHover { phase in
                    switch phase {
                    case .active(let location):
                        hovering = true
                        guard !reduceMotion else { return }
                        pointer = CGPoint(
                            x: min(1, max(0, location.x / max(1, geometry.size.width))),
                            y: min(1, max(0, location.y / max(1, geometry.size.height)))
                        )
                    case .ended:
                        withAnimation(.spring(response: 0.42, dampingFraction: 0.82)) {
                            hovering = false
                            pointer = CGPoint(x: 0.5, y: 0.5)
                        }
                    }
                }
                .gesture(
                    DragGesture(minimumDistance: 2)
                        .onChanged { value in
                            guard !reduceMotion else { return }
                            drag = value.translation
                        }
                        .onEnded { _ in
                            withAnimation(.spring(response: 0.5, dampingFraction: 0.72)) {
                                drag = .zero
                            }
                        }
                )
                .onTapGesture {
                    withAnimation(.spring(response: 0.46, dampingFraction: 0.78)) {
                        revealed.toggle()
                    }
                }
                .help(AppLocalization.string("installCard.moveDragOrClickTheCard"))
            }
            .aspectRatio(1.6, contentMode: .fit)

            HStack(spacing: 10) {
                Label(
                    AppLocalization.string("installCard.moveDragAndClick"),
                    systemImage: "cursorarrow.motionlines"
                )
                .font(.system(size: 12))
                .foregroundStyle(.secondary)

                Spacer()

                Button {
                    onSave(revealed, wallpaperOffset)
                } label: {
                    Label(AppLocalization.string("installCard.savePNG"), systemImage: "square.and.arrow.down")
                }

                Button {
                    onShare(revealed, wallpaperOffset)
                } label: {
                    Label(AppLocalization.string("installCard.share"), systemImage: "square.and.arrow.up")
                }
                .buttonStyle(.borderedProminent)
            }
        }
        .padding(28)
        .frame(minWidth: 720, minHeight: 520)
    }
}

struct InstallCardThumbnailView: View {
    let identity: CardIdentityV1
    let wallpaperOffset: Int
    let onOpen: () -> Void

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var hovering = false

    var body: some View {
        Button(action: onOpen) {
            InstallCardSurface(
                identity: identity,
                wallpaperOffset: wallpaperOffset,
                elapsed: 0,
                pointer: hovering && !reduceMotion
                    ? CGPoint(x: 0.58, y: 0.42)
                    : CGPoint(x: 0.5, y: 0.5),
                drag: .zero,
                revealed: false,
                now: Date()
            )
        }
        .buttonStyle(.plain)
        .scaleEffect(hovering && !reduceMotion ? 1.018 : 1)
        .shadow(
            color: .black.opacity(hovering ? 0.32 : 0.2),
            radius: hovering ? 18 : 10,
            y: hovering ? 10 : 6
        )
        .animation(.spring(response: 0.34, dampingFraction: 0.82), value: hovering)
        .onHover { hovering = $0 }
        .help(AppLocalization.string("installCard.clickToEnlargeTheMixinCard"))
        .accessibilityLabel(AppLocalization.string("installCard.openMyMixinCard"))
    }
}

@MainActor
func renderInstallCardPNG(
    identity: CardIdentityV1,
    revealed: Bool,
    wallpaperOffset: Int = 0,
    now: Date = Date(),
    size: CGSize = CGSize(width: 1_200, height: 750)
) -> Data? {
    let surface = InstallCardSurface(
        identity: identity,
        wallpaperOffset: wallpaperOffset,
        elapsed: 0,
        pointer: CGPoint(x: 0.5, y: 0.5),
        drag: .zero,
        revealed: revealed,
        now: now
    )
    .frame(width: size.width, height: size.height)

    let renderer = ImageRenderer(content: surface)
    renderer.proposedSize = ProposedViewSize(size)
    renderer.scale = 1
    guard
        let image = renderer.nsImage,
        let tiff = image.tiffRepresentation,
        let bitmap = NSBitmapImageRep(data: tiff)
    else {
        return nil
    }
    return bitmap.representation(using: .png, properties: [:])
}

final class InstallCardWindowController: NSWindowController, NSWindowDelegate {
    private let identity: CardIdentityV1
    private let wallpaperOffset: Int
    private var sharingPicker: NSSharingServicePicker?

    init(
        identityStore: CardIdentityStore = .standard,
        wallpaperOffset: Int = 0
    ) {
        identity = identityStore.current()
        self.wallpaperOffset = wallpaperOffset
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 820, height: 610),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = AppLocalization.string("installCard.myMixinCard")
        window.minSize = NSSize(width: 720, height: 560)
        window.center()

        super.init(window: window)
        window.delegate = self
        installContent(in: window)
        configurePersistentWindow(window)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func present() {
        showWindow(nil)
        if let window {
            presentPersistentWindow(window)
        }
    }

    private func installContent(in window: NSWindow) {
        let rootView = InstallCardExperienceView(
            identity: identity,
            wallpaperOffset: wallpaperOffset,
            onSave: { [weak self] revealed, wallpaperOffset in
                self?.savePNG(revealed: revealed, wallpaperOffset: wallpaperOffset)
            },
            onShare: { [weak self] revealed, wallpaperOffset in
                self?.sharePNG(revealed: revealed, wallpaperOffset: wallpaperOffset)
            }
        )
        window.contentViewController = NSHostingController(rootView: rootView)
    }

    private func savePNG(revealed: Bool, wallpaperOffset: Int) {
        guard
            let window,
            let data = renderInstallCardPNG(
                identity: identity,
                revealed: revealed,
                wallpaperOffset: wallpaperOffset
            )
        else {
            showAlert(
                title: AppLocalization.string("installCard.couldNotRenderCard"),
                message: AppLocalization.string("installCard.pleaseTryAgain")
            )
            return
        }

        let panel = NSSavePanel()
        panel.allowedContentTypes = [.png]
        panel.canCreateDirectories = true
        panel.nameFieldStringValue = "codex-mixin-card.png"
        panel.beginSheetModal(for: window) { response in
            guard response == .OK, let url = panel.url else { return }
            do {
                try data.write(to: url, options: .atomic)
            } catch {
                showAlert(
                    title: AppLocalization.string("installCard.saveFailed"),
                    message: String(describing: error)
                )
            }
        }
    }

    private func sharePNG(revealed: Bool, wallpaperOffset: Int) {
        guard
            let window,
            let anchor = window.contentView,
            let data = renderInstallCardPNG(
                identity: identity,
                revealed: revealed,
                wallpaperOffset: wallpaperOffset
            )
        else {
            showAlert(
                title: AppLocalization.string("installCard.couldNotRenderCard2"),
                message: AppLocalization.string("installCard.pleaseTryAgain2")
            )
            return
        }

        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("codex-mixin-card-\(UUID().uuidString).png")
        do {
            try data.write(to: url, options: .atomic)
            let picker = NSSharingServicePicker(items: [url])
            sharingPicker = picker
            picker.show(
                relativeTo: NSRect(x: anchor.bounds.midX, y: 0, width: 1, height: 1),
                of: anchor,
                preferredEdge: .minY
            )
        } catch {
            showAlert(
                title: AppLocalization.string("installCard.shareFailed"),
                message: String(describing: error)
            )
        }
    }
}
