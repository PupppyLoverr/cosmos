import XCTest
@testable import Zeron

final class WebLinkTests: XCTestCase {
    func testAcceptsExplicitHttpAndHttps() {
        XCTAssertEqual(WebLink.transcriptURL("https://example.com")?.host, "example.com")
        XCTAssertEqual(WebLink.transcriptURL("HTTP://example.com")?.scheme, "http")
        XCTAssertEqual(WebLink.transcriptURL("HTTPS://EXAMPLE.COM/path?q=1")?.host?.lowercased(), "example.com")
        XCTAssertNotNil(WebLink.transcriptURL("https://github.com/zeron-sh/zeron/pull/90"))
    }

    func testRejectsNonWebSchemesCredentialsAndMalformedInput() {
        for url in [
            "javascript:alert(1)",
            "data:text/plain,hi",
            "mailto:a@b.com",
            "https://user:pass@example.com",
            "https://example.com/\npath",
            "\nhttps://example.com",
            "https://",
            "example.com",
            "https:///path",
            "https://example.com/%GG",
            "file:///tmp/a",
            "zeron://open/chat/a",
        ] {
            XCTAssertNil(WebLink.transcriptURL(url), url)
        }
    }

    func testInPageNavigationStillBlocksCredentials() {
        XCTAssertTrue(WebLink.allowsNavigation(URL(string: "https://example.com/next")!))
        XCTAssertFalse(WebLink.allowsNavigation(URL(string: "https://user:pass@example.com")!))
        XCTAssertFalse(WebLink.allowsNavigation(URL(string: "javascript:alert(1)")!))
    }
}
