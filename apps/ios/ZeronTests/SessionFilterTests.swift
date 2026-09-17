import XCTest
@testable import Zeron

final class SessionFilterTests: XCTestCase {
    private func chat(
        title: String,
        preview: String? = "hello",
        unseen: Bool = false,
        harness: String = "claude-code",
        branch: String? = "main",
        cwd: String? = "/Users/dev/zeron",
        spaceId: String? = "space-zeron"
    ) -> Chat {
        Chat(id: title, deviceId: "dev-mac", title: title, archived: false,
             cwd: cwd, branch: branch, checkoutId: nil,
             config: ChatConfig(harness: harness, model: "claude-fable-5",
                                reasoning: "high", sandbox: "workspace-write"),
             lastMessagePreview: preview, lastMessageAt: unseen ? 20 : 10,
             createdAt: 1, spaceId: spaceId, lastSeenAt: 10)
    }

    func testAttentionIncludesInputErrorsAndFailedDelivery() {
        let idle = chat(title: "idle")
        XCTAssertTrue(SessionFilter.all.includes(idle, indicator: .idle, deliveryFailed: false))
        XCTAssertFalse(SessionFilter.attention.includes(idle, indicator: .idle, deliveryFailed: false))
        XCTAssertTrue(SessionFilter.attention.includes(idle, indicator: .awaitingInput, deliveryFailed: false))
        XCTAssertTrue(SessionFilter.attention.includes(idle, indicator: .errored, deliveryFailed: false))
        XCTAssertTrue(SessionFilter.attention.includes(idle, indicator: .idle, deliveryFailed: true))
        XCTAssertTrue(SessionFilter.running.includes(idle, indicator: .working, deliveryFailed: false))
        XCTAssertFalse(SessionFilter.running.includes(idle, indicator: .idle, deliveryFailed: false))
    }

    func testUnreadUsesTheSeenMarker() {
        let read = chat(title: "read", unseen: false)
        let unread = chat(title: "unread", unseen: true)
        XCTAssertFalse(SessionFilter.unread.includes(read, indicator: .completed, deliveryFailed: false))
        XCTAssertTrue(SessionFilter.unread.includes(unread, indicator: .completed, deliveryFailed: false))
    }

    func testSearchMatchesEveryWordAcrossTitlePreviewProjectAndDevice() {
        let chat = chat(title: "Streaming veil", preview: "Porting the paint-only fade")
        XCTAssertTrue(chat.matchesSessionSearch("", project: "zeron", device: "MacBook Pro"))
        XCTAssertTrue(chat.matchesSessionSearch("veil fade", project: "zeron", device: "MacBook Pro"))
        XCTAssertTrue(chat.matchesSessionSearch("macbook claude", project: "zeron", device: "MacBook Pro"))
        XCTAssertTrue(chat.matchesSessionSearch("main zeron", project: "zeron", device: "MacBook Pro"))
        XCTAssertFalse(chat.matchesSessionSearch("veil missing-term", project: "zeron", device: "MacBook Pro"))
    }

    func testNewSessionDraftKeysAreStablePerDestination() {
        XCTAssertEqual(NewSessionDestination.project(spaceId: "abc").draftKey, "project.abc")
        XCTAssertEqual(NewSessionDestination.projectless(deviceId: "mac").draftKey, "host.mac")
    }
}

@MainActor
final class ComposerDraftTests: XCTestCase {
    func testComposerDraftReloadsFromUserDefaults() {
        let chatId = "draft-\(UUID().uuidString)"
        let config = AppConfig(edgeURL: URL(string: "http://localhost:1")!, mode: .dev,
                               userId: "draft-tests", orgId: "tests", deviceId: "ios-test",
                               deviceName: "Test phone", devBearer: "draft-tests@tests")
        let store = SessionStore(chatId: chatId, config: config, offline: true)
        store.composerDraft = "keep me around"
        let restored = SessionStore(chatId: chatId, config: config, offline: true)
        XCTAssertEqual(restored.composerDraft, "keep me around")
        restored.clearComposerDraft()
        let empty = SessionStore(chatId: chatId, config: config, offline: true)
        XCTAssertEqual(empty.composerDraft, "")
    }
}
