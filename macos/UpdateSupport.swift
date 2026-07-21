import Cocoa

struct GitHubRelease: Decodable {
    let tagName: String
    let htmlURL: URL
    let body: String?
    let assets: [Asset]

    var version: String {
        tagName.hasPrefix("v") ? String(tagName.dropFirst()) : tagName
    }

    enum CodingKeys: String, CodingKey {
        case tagName = "tag_name"
        case htmlURL = "html_url"
        case body
        case assets
    }

    func localizedNotes(language: UpdateLanguage, fallback: String) -> String {
        guard let body, !body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            return fallback
        }
        let selected = releaseNotesSection(body, key: language.releaseNotesKey)
            ?? releaseNotesSection(body, key: UpdateLanguage.english.releaseNotesKey)
            ?? body
        let rendered = readableReleaseNotes(selected)
        return rendered.isEmpty ? fallback : rendered
    }

    struct Asset: Decodable {
        let name: String
        let browserDownloadURL: URL

        enum CodingKeys: String, CodingKey {
            case name
            case browserDownloadURL = "browser_download_url"
        }
    }
}

enum UpdatePromptAction {
    case download
    case releasePage
    case later
}

enum UpdateLanguage {
    case simplifiedChinese
    case traditionalChinese
    case english

    static var current: UpdateLanguage {
        let preferred = Locale.preferredLanguages.first?.lowercased() ?? "en"
        if preferred.hasPrefix("zh-hant")
            || preferred.hasPrefix("zh-tw")
            || preferred.hasPrefix("zh-hk")
            || preferred.hasPrefix("zh-mo")
        {
            return .traditionalChinese
        }
        return preferred.hasPrefix("zh") ? .simplifiedChinese : .english
    }

    var releaseNotesKey: String {
        switch self {
        case .simplifiedChinese: return "zh-Hans"
        case .traditionalChinese: return "zh-Hant"
        case .english: return "en"
        }
    }
}

struct UpdateStrings {
    let language: UpdateLanguage

    static var current: UpdateStrings {
        UpdateStrings(language: .current)
    }

    var checkFailedTitle: String {
        switch language {
        case .simplifiedChinese: return "检查更新失败"
        case .traditionalChinese: return "檢查更新失敗"
        case .english: return "Unable to Check for Updates"
        }
    }

    var downloadFailedTitle: String {
        switch language {
        case .simplifiedChinese: return "下载更新失败"
        case .traditionalChinese: return "下載更新失敗"
        case .english: return "Unable to Download Update"
        }
    }

    var upToDateTitle: String {
        switch language {
        case .simplifiedChinese: return "已经是最新版本"
        case .traditionalChinese: return "已是最新版本"
        case .english: return "Codex Mixin Is Up to Date"
        }
    }

    func upToDateMessage(current: String, latest: String) -> String {
        switch language {
        case .simplifiedChinese: return "当前版本 \(current)，最新版本 \(latest)。"
        case .traditionalChinese: return "目前版本 \(current)，最新版本 \(latest)。"
        case .english: return "Current version: \(current). Latest version: \(latest)."
        }
    }

    func updateAvailableTitle(version: String) -> String {
        switch language {
        case .simplifiedChinese: return "Codex Mixin \(version) 可用"
        case .traditionalChinese: return "Codex Mixin \(version) 可供更新"
        case .english: return "Codex Mixin \(version) Is Available"
        }
    }

    func versionSummary(current: String, latest: String, assetAvailable: Bool) -> String {
        switch (language, assetAvailable) {
        case (.simplifiedChinese, true):
            return "当前版本 \(current) → 新版本 \(latest)。请查看下面的版本变更。"
        case (.simplifiedChinese, false):
            return "当前版本 \(current) → 新版本 \(latest)。暂未找到适合当前 Mac 的 DMG，可前往 Release 页面下载。"
        case (.traditionalChinese, true):
            return "目前版本 \(current) → 新版本 \(latest)。請查看下方的版本變更。"
        case (.traditionalChinese, false):
            return "目前版本 \(current) → 新版本 \(latest)。暫未找到適合目前 Mac 的 DMG，可前往 Release 頁面下載。"
        case (.english, true):
            return "Current version \(current) → new version \(latest). Review the changes below."
        case (.english, false):
            return "Current version \(current) → new version \(latest). No matching DMG was found for this Mac; use the Release page instead."
        }
    }

    var whatsNewTitle: String {
        switch language {
        case .simplifiedChinese: return "版本变更"
        case .traditionalChinese: return "版本變更"
        case .english: return "What's New"
        }
    }

    var noReleaseNotes: String {
        switch language {
        case .simplifiedChinese: return "此版本暂未提供变更说明，请打开 Release 页面查看详情。"
        case .traditionalChinese: return "此版本暫未提供變更說明，請開啟 Release 頁面查看詳情。"
        case .english: return "No release notes were provided. Open the Release page for details."
        }
    }

    var downloadButton: String {
        switch language {
        case .simplifiedChinese: return "下载更新"
        case .traditionalChinese: return "下載更新"
        case .english: return "Download Update"
        }
    }

    var releasePageButton: String {
        switch language {
        case .simplifiedChinese: return "查看 Release 页面"
        case .traditionalChinese: return "查看 Release 頁面"
        case .english: return "View Release Page"
        }
    }

    var laterButton: String {
        switch language {
        case .simplifiedChinese: return "稍后"
        case .traditionalChinese: return "稍後"
        case .english: return "Later"
        }
    }
}


func compareVersions(_ lhs: String, _ rhs: String) -> ComparisonResult {
    let leftParts = lhs.split(separator: ".").map { Int($0) ?? 0 }
    let rightParts = rhs.split(separator: ".").map { Int($0) ?? 0 }
    let count = max(leftParts.count, rightParts.count)
    for index in 0..<count {
        let left = index < leftParts.count ? leftParts[index] : 0
        let right = index < rightParts.count ? rightParts[index] : 0
        if left < right {
            return .orderedAscending
        }
        if left > right {
            return .orderedDescending
        }
    }
    return .orderedSame
}

func releaseNotesSection(_ body: String, key: String) -> String? {
    let startMarker = "<!-- codex-mixin:\(key):start -->"
    let endMarker = "<!-- codex-mixin:\(key):end -->"
    guard let start = body.range(of: startMarker) else { return nil }
    guard let end = body.range(of: endMarker, range: start.upperBound..<body.endIndex) else {
        return nil
    }
    return String(body[start.upperBound..<end.lowerBound])
        .trimmingCharacters(in: .whitespacesAndNewlines)
}

func readableReleaseNotes(_ markdown: String) -> String {
    let linkExpression = try? NSRegularExpression(pattern: #"\[([^\]]+)\]\([^)]+\)"#)
    var rendered: [String] = []
    var previousWasBlank = false
    for rawLine in markdown.components(separatedBy: .newlines) {
        var line = rawLine.trimmingCharacters(in: .whitespaces)
        if line.hasPrefix("<!--") || line == "---" || line == "***" {
            continue
        }
        while line.hasPrefix("#") {
            line.removeFirst()
        }
        line = line.trimmingCharacters(in: .whitespaces)
        if line.hasPrefix("- ") || line.hasPrefix("* ") {
            line = "• " + line.dropFirst(2)
        }
        if let linkExpression {
            let range = NSRange(line.startIndex..<line.endIndex, in: line)
            line = linkExpression.stringByReplacingMatches(
                in: line,
                range: range,
                withTemplate: "$1"
            )
        }
        for marker in ["**", "__", "`"] {
            line = line.replacingOccurrences(of: marker, with: "")
        }
        let isBlank = line.isEmpty
        if isBlank && previousWasBlank {
            continue
        }
        rendered.append(line)
        previousWasBlank = isBlank
    }
    let result = rendered.joined(separator: "\n")
        .trimmingCharacters(in: .whitespacesAndNewlines)
    guard result.count > 12_000 else { return result }
    return String(result.prefix(12_000)) + "\n…"
}

func releaseNotesView(title: String, notes: String) -> NSView {
    let width: CGFloat = 560
    let height: CGFloat = 300
    let titleHeight: CGFloat = 24
    let container = NSView(frame: NSRect(x: 0, y: 0, width: width, height: height))
    let titleLabel = NSTextField(labelWithString: title)
    titleLabel.font = .boldSystemFont(ofSize: 13)
    titleLabel.frame = NSRect(x: 0, y: height - titleHeight, width: width, height: titleHeight)

    let scrollView = NSScrollView(
        frame: NSRect(x: 0, y: 0, width: width, height: height - titleHeight - 4)
    )
    scrollView.hasVerticalScroller = true
    scrollView.autohidesScrollers = true
    scrollView.borderType = .bezelBorder

    let textView = NSTextView(frame: scrollView.contentView.bounds)
    textView.string = notes
    textView.font = .systemFont(ofSize: 13)
    textView.textColor = .labelColor
    textView.backgroundColor = .textBackgroundColor
    textView.isEditable = false
    textView.isSelectable = true
    textView.isRichText = false
    textView.isHorizontallyResizable = false
    textView.isVerticallyResizable = true
    textView.autoresizingMask = [.width]
    textView.textContainerInset = NSSize(width: 10, height: 10)
    textView.textContainer?.widthTracksTextView = true
    textView.textContainer?.containerSize = NSSize(
        width: scrollView.contentSize.width,
        height: .greatestFiniteMagnitude
    )
    textView.layoutManager?.ensureLayout(for: textView.textContainer!)
    let usedHeight = textView.layoutManager?
        .usedRect(for: textView.textContainer!)
        .height ?? scrollView.contentSize.height
    textView.frame.size.height = max(
        scrollView.contentSize.height,
        usedHeight + textView.textContainerInset.height * 2
    )
    scrollView.documentView = textView
    container.addSubview(titleLabel)
    container.addSubview(scrollView)
    return container
}

