import Foundation

enum SessionFilter: String, CaseIterable, Identifiable {
    case all, attention, running, unread

    var id: String { rawValue }
    var title: String {
        switch self {
        case .all: "All sessions"
        case .attention: "Needs you"
        case .running: "Running"
        case .unread: "Unread"
        }
    }

    func includes(_ chat: Chat, indicator: ChatIndicator, deliveryFailed: Bool) -> Bool {
        switch self {
        case .all: true
        case .attention: indicator == .awaitingInput || indicator == .errored || deliveryFailed
        case .running: indicator == .working
        case .unread: chat.unseen
        }
    }
}

extension Chat {
    /// Search the metadata already on the phone, without opening every transcript.
    /// Every word must match, but words may occur in different fields.
    func matchesSessionSearch(_ query: String, project: String, device: String) -> Bool {
        let terms = query.split(whereSeparator: \.isWhitespace)
        let fields = [displayTitle, lastMessagePreview ?? "", branch ?? "", cwd ?? "",
                      config?.harness ?? "", config?.model ?? "", project, device]
        return terms.allSatisfy { term in
            fields.contains { $0.localizedStandardContains(String(term)) }
        }
    }
}
