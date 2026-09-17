import Foundation

/// Transcript and in-app browser destinations. Matches desktop
/// `transcript_address`: only explicit HTTP(S) URLs without credentials,
/// control characters, whitespace, backslashes, or malformed percent escapes.
/// Rejected links never fall through to Safari.
enum WebLink {
    static func transcriptURL(_ input: String) -> URL? {
        let lower = input.lowercased()
        let rest: Substring
        if lower.hasPrefix("https://") {
            rest = lower.dropFirst(8)
        } else if lower.hasPrefix("http://") {
            rest = lower.dropFirst(7)
        } else {
            return nil
        }
        if input.contains(where: { character in
            character.unicodeScalars.contains { $0.properties.isISOControl || $0.properties.isWhitespace }
        }) || input.contains("\\") {
            return nil
        }
        if rest.isEmpty || rest.hasPrefix("/") || rest.hasPrefix("?") || rest.hasPrefix("#") {
            return nil
        }
        let bytes = Array(input.utf8)
        for index in bytes.indices where bytes[index] == UInt8(ascii: "%") {
            guard index + 2 < bytes.count,
                  isHex(bytes[index + 1]), isHex(bytes[index + 2]) else { return nil }
        }
        guard let url = URL(string: input), allowsNavigation(url) else { return nil }
        return url
    }

    static func transcriptURL(_ url: URL) -> URL? {
        transcriptURL(url.absoluteString)
    }

    /// In-page navigations inside the in-app browser: still HTTP(S) only,
    /// still no embedded credentials, but the address may be a redirect.
    static func allowsNavigation(_ url: URL) -> Bool {
        guard let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https" else {
            return false
        }
        guard let host = url.host, !host.isEmpty else { return false }
        if let user = url.user, !user.isEmpty { return false }
        if url.password != nil { return false }
        return true
    }

    private static func isHex(_ byte: UInt8) -> Bool {
        (byte >= 48 && byte <= 57) || (byte >= 65 && byte <= 70) || (byte >= 97 && byte <= 102)
    }
}
