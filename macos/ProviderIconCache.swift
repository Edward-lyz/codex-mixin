import Cocoa

enum ProviderIconCacheError: LocalizedError {
    case invalidWebsiteURL(String)
    case httpStatus(Int)
    case invalidImage

    var errorDescription: String? {
        switch self {
        case let .invalidWebsiteURL(value):
            return "无效的 Provider 官网地址：\(value)"
        case let .httpStatus(status):
            return "Provider 图标请求返回 HTTP \(status)"
        case .invalidImage:
            return "Provider 官网没有返回可用图标"
        }
    }
}

private func providerIconCacheURL(providerID: String, websiteURL: URL) -> URL {
    let allowed = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "-_"))
    let providerComponent = providerID.unicodeScalars
        .map { allowed.contains($0) ? String($0) : "_" }
        .joined()
    let hostComponent = (websiteURL.host ?? "site").unicodeScalars
        .map { allowed.contains($0) ? String($0) : "_" }
        .joined()
    return FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent(".codex-mixin/provider-icons", isDirectory: true)
        .appendingPathComponent("v2-\(providerComponent)-\(hostComponent).png")
}

func cachedProviderLogoImage(providerID: String, websiteURL: String?) -> NSImage? {
    guard let websiteURL, let url = URL(string: websiteURL) else { return nil }
    return NSImage(contentsOf: providerIconCacheURL(providerID: providerID, websiteURL: url))
}

func declaredFaviconURL(html: String, baseURL: URL) -> URL? {
    let linkPattern = #"(?is)<link\b[^>]*\brel\s*=\s*[\"'][^\"']*icon[^\"']*[\"'][^>]*>"#
    let hrefPattern = #"(?is)\bhref\s*=\s*[\"']([^\"']+)[\"']"#
    guard
        let linkExpression = try? NSRegularExpression(pattern: linkPattern),
        let hrefExpression = try? NSRegularExpression(pattern: hrefPattern)
    else {
        return nil
    }
    let htmlRange = NSRange(html.startIndex..., in: html)
    for linkMatch in linkExpression.matches(in: html, range: htmlRange) {
        guard let matchedLinkRange = Range(linkMatch.range, in: html) else { continue }
        let link = String(html[matchedLinkRange])
        let linkSearchRange = NSRange(link.startIndex..., in: link)
        guard
            let hrefMatch = hrefExpression.firstMatch(in: link, range: linkSearchRange),
            hrefMatch.numberOfRanges == 2,
            let hrefRange = Range(hrefMatch.range(at: 1), in: link)
        else {
            continue
        }
        return URL(string: String(link[hrefRange]), relativeTo: baseURL)?.absoluteURL
    }
    return nil
}

func embeddedProviderIconData(from url: URL?) -> Data? {
    guard
        let url,
        url.scheme?.lowercased() == "data",
        let commaIndex = url.absoluteString.firstIndex(of: ",")
    else {
        return nil
    }
    let metadata = url.absoluteString[..<commaIndex].lowercased()
    guard metadata.hasPrefix("data:image/"), metadata.hasSuffix(";base64") else {
        return nil
    }
    let encodedData = String(url.absoluteString[url.absoluteString.index(after: commaIndex)...])
    guard let base64 = encodedData.removingPercentEncoding else { return nil }
    return Data(base64Encoded: base64, options: .ignoreUnknownCharacters)
}

private func providerIconPNG(from data: Data) -> Data? {
    guard
        let image = NSImage(data: data),
        let tiff = image.tiffRepresentation,
        let bitmap = NSBitmapImageRep(data: tiff)
    else {
        return nil
    }
    return bitmap.representation(using: .png, properties: [:])
}

func refreshProviderLogoIfNeeded(providerID: String, websiteURL: String) async throws -> Bool {
    guard
        let siteURL = URL(string: websiteURL),
        let scheme = siteURL.scheme?.lowercased(),
        scheme == "https" || scheme == "http",
        siteURL.host != nil
    else {
        throw ProviderIconCacheError.invalidWebsiteURL(websiteURL)
    }

    let cacheURL = providerIconCacheURL(providerID: providerID, websiteURL: siteURL)
    if let attributes = try? FileManager.default.attributesOfItem(atPath: cacheURL.path),
       let modifiedAt = attributes[.modificationDate] as? Date,
       Date().timeIntervalSince(modifiedAt) < 7 * 24 * 60 * 60
    {
        return false
    }

    var fallbackComponents = URLComponents(url: siteURL, resolvingAgainstBaseURL: false)
    fallbackComponents?.path = "/favicon.ico"
    fallbackComponents?.query = nil
    fallbackComponents?.fragment = nil
    guard let fallbackURL = fallbackComponents?.url else {
        throw ProviderIconCacheError.invalidWebsiteURL(websiteURL)
    }

    let configuration = URLSessionConfiguration.ephemeral
    configuration.timeoutIntervalForRequest = 5
    configuration.timeoutIntervalForResource = 8
    let session = URLSession(configuration: configuration)
    var candidates: [URL] = []
    if let (pageData, response) = try? await session.data(from: siteURL),
       let response = response as? HTTPURLResponse,
       (200 ..< 300).contains(response.statusCode),
       pageData.count <= 2 * 1024 * 1024,
       let html = String(data: pageData, encoding: .utf8),
       let declaredURL = declaredFaviconURL(
           html: html,
           baseURL: response.url ?? siteURL
       )
    {
        candidates.append(declaredURL)
    }
    if !candidates.contains(fallbackURL) {
        candidates.append(fallbackURL)
    }
    if let host = siteURL.host,
       let proxyURL = URL(string: "https://favicon.im/\(host)")
    {
        candidates.append(proxyURL)
    }

    var lastHTTPStatus: Int?
    var png: Data?
    for candidate in candidates {
        do {
            let data: Data
            if let embeddedData = embeddedProviderIconData(from: candidate) {
                data = embeddedData
            } else {
                let (downloadedData, response) = try await session.data(from: candidate)
                if let response = response as? HTTPURLResponse,
                   !(200 ..< 300).contains(response.statusCode)
                {
                    lastHTTPStatus = response.statusCode
                    continue
                }
                data = downloadedData
            }
            if let candidatePNG = providerIconPNG(from: data) {
                png = candidatePNG
                break
            }
        } catch {
            continue
        }
    }
    guard let png else {
        if let lastHTTPStatus {
            throw ProviderIconCacheError.httpStatus(lastHTTPStatus)
        }
        throw ProviderIconCacheError.invalidImage
    }

    try FileManager.default.createDirectory(
        at: cacheURL.deletingLastPathComponent(),
        withIntermediateDirectories: true
    )
    try png.write(to: cacheURL, options: .atomic)
    return true
}
