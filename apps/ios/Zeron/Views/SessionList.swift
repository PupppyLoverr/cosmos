import SwiftUI

/// Shared session inventory for Home and a Space: search, attention filters,
/// swipe/context actions, and the archived shelf.
struct SessionList: View {
    @Environment(AppModel.self) private var model
    var spaceId: String?
    @Binding var path: [Route]
    var onStartSession: () -> Void

    @State private var searchText = ""
    @State private var sessionFilter: SessionFilter = .all

    private var scopedChats: [Chat] {
        spaceId.map { model.chats(in: $0) } ?? model.overviewChats
    }

    private var matchingChats: [Chat] {
        scopedChats.filter {
            model.matchesSearch($0, query: searchText)
                && sessionFilter.includes($0, indicator: model.indicator(for: $0),
                                          deliveryFailed: model.sendState(for: $0) == .failed)
        }
    }

    private var matchingArchived: [Chat] {
        guard sessionFilter == .all else { return [] }
        return model.archivedChats(in: spaceId).filter { model.matchesSearch($0, query: searchText) }
    }

    var body: some View {
        List {
            sessionsSection
            if sessionFilter == .all {
                ArchivedSection(spaceId: spaceId, path: $path, searchText: searchText)
            }
        }
        .safeAreaInset(edge: .top, spacing: 0) { filterBar }
        .searchable(text: $searchText, placement: .navigationBarDrawer(displayMode: .always),
                    prompt: "Search sessions, projects, or devices")
        .autocorrectionDisabled()
        .refreshable { await model.refreshOverview() }
        .scrollDismissesKeyboard(.interactively)
        .listStyle(.plain)
        .environment(\.defaultMinListRowHeight, 10)
        .contentMargins(.top, 2, for: .scrollContent)
        .scrollContentBackground(.hidden)
        .scrollEdgeEffectStyle(.soft, for: .top)
        .background(Theme.bg.ignoresSafeArea())
        .accessibilityIdentifier("session-list")
    }

    private var filterBar: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 18) {
                ForEach(SessionFilter.allCases) { filter in }
                    let count = scopedChats.filter {
                        model.matchesSearch($0, query: searchText)
                            && filter.includes($0, indicator: model.indicator(for: $0),
                                               deliveryFailed: model.sendState(for: $0) == .failed)
                    }.count
                    Button {
                        sessionFilter = filter
                    } label: {
                        HStack(spacing: 6) {
                            Text(filter.title)
                            Text(count, format: .number).monospacedDigit()
                                .foregroundStyle(Theme.textMuted)
                        }
                        .font(Theme.sans(13, weight: sessionFilter == filter ? .semibold : .regular))
                        .padding(.horizontal, 4)
                        .frame(minHeight: 36)
                        .overlay(alignment: .bottom) {
                            if sessionFilter == filter {
                                Capsule()
                                    .fill(Theme.text)
                                    .frame(width: 14, height: 2)
                                    .padding(.bottom, 6)
                            }
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(sessionFilter == filter ? Theme.text : Theme.textMuted)
                    .accessibilityLabel("\(filter.title), \(count) sessions")
                    .accessibilityAddTraits(sessionFilter == filter ? .isSelected : [])
                    .accessibilityIdentifier("session-filter-\(filter.rawValue)")
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 2)
        }
        .background(Theme.bg)
        .overlay(alignment: .bottom) {
            Theme.border.frame(height: 1)
        }
    }

    private var emptySessions: some View {
        let searching = !searchText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        return VStack(spacing: 12) {
            Image(systemName: searching ? "magnifyingglass"
                  : sessionFilter == .attention ? "checkmark.circle"
                  : "bubble.left.and.bubble.right")
                .font(.system(size: 28, weight: .light))
                .foregroundStyle(Theme.textMuted)
            Text(searching ? "No matching sessions"
                 : sessionFilter == .attention ? "You're all caught up"
                 : sessionFilter == .all ? "Start a conversation"
                 : "No \(sessionFilter.title.lowercased()) sessions")
                .font(Theme.sans(18, weight: .medium))
                .foregroundStyle(Theme.text)
            Text(searching ? "Try a title, branch, project, or device name."
                 : sessionFilter == .all ? "Choose a project or a device to start working with an agent."
                 : "Sessions will appear here when their status changes.")
                .font(Theme.sans(14))
                .foregroundStyle(Theme.textMuted)
                .multilineTextAlignment(.center)
            if searching || sessionFilter != .all {
                Button("Show all sessions") {
                    searchText = ""
                    sessionFilter = .all
                }
                .frame(minHeight: 44)
            } else {
                Button("New session", action: onStartSession)
                    .frame(minHeight: 44)
                    .accessibilityIdentifier("empty-new-session")
            }
        }
        .padding(.horizontal, 24)
        .padding(.vertical, 32)
        .frame(maxWidth: .infinity)
        .accessibilityIdentifier("sessions-empty-state")
    }

    private var sessionsSection: some View {
        Section {
            let chats = matchingChats
            if chats.isEmpty && matchingArchived.isEmpty {
                emptySessions
                    .listRowBackground(Color.clear)
                    .listRowSeparator(.hidden)
            }
            ForEach(chats) { chat in
                ChatRow(chat: chat, showLocation: true) {
                    path.append(.chat(chat.id))
                }
                .listRowBackground(Color.clear)
                .listRowSeparator(.hidden)
                .listRowInsets(EdgeInsets(top: 1, leading: 12, bottom: 1, trailing: 12))
            }
            .motionAnimation(Motion.resort, value: chats.map(\.id))
        }
    }
}

// MARK: - Rows

/// The desktop session row (shell.rs `render_chat_row`), line for line: a
/// muted context line with the status word in the corner (dot + word, muted;
/// Done keeps its pop with a check; Idle rows carry the time-ago there
/// instead); the title on its own line; harness mark and branch close it out,
/// with the mini spinner riding the row's bottom-right while Working.
///
/// Phone additions: location (`space @ device`) because the list interleaves
/// every host; last-message preview so a long list is scannable; swipe to
/// archive / mark read.
struct ChatRow: View {
    @Environment(AppModel.self) private var model
    let chat: Chat
    var showLocation: Bool
    let onSelect: () -> Void

    private var subline: Color { Theme.textMuted.opacity(0.5) }

    var body: some View {
        let _ = model.connectivity.pulse
        let indicator = model.indicator(for: chat)
        let sendState = model.sendState(for: chat)
        let pullRequest = model.changeRequest(for: chat)
        ZStack(alignment: .bottomTrailing) {
            Button(action: onSelect) {
                content(indicator: indicator, sendState: sendState,
                        reservesPullRequest: pullRequest != nil)
            }
            .buttonStyle(PressWashButtonStyle())
            if let pullRequest {
                PullRequestBadge(summary: pullRequest)
                    .padding(.trailing, 8)
                    .padding(.bottom, 6)
                    .zIndex(1)
            }
        }
        .sessionRowActions(chat: chat)
        .contextMenu {
            Button {
                onSelect()
            } label: {
                Label("Open", systemImage: "bubble.left")
            }
            if chat.unseen {
                Button {
                    model.markSeen(chatId: chat.id)
                } label: {
                    Label("Mark as read", systemImage: "checkmark.circle")
                }
            }
            Button(role: chat.archived ? .none : .destructive) {
                withAnimation(Motion.resort) {
                    if chat.archived { model.unarchive(chatId: chat.id) }
                    else { model.archive(chatId: chat.id) }
                }
            } label: {
                Label(chat.archived ? "Unarchive" : "Archive",
                      systemImage: chat.archived ? "arrow.up.bin" : "archivebox")
            }
        }
        .swipeActions(edge: .trailing, allowsFullSwipe: true) {
            Button {
                withAnimation(Motion.resort) {
                    if chat.archived { model.unarchive(chatId: chat.id) }
                    else { model.archive(chatId: chat.id) }
                }
            } label: {
                Label(chat.archived ? "Unarchive" : "Archive",
                      systemImage: chat.archived ? "arrow.up.bin" : "archivebox")
            }
            .tint(Theme.surfaceRaised)
        }
        .swipeActions(edge: .leading) {
            if chat.unseen {
                Button {
                    model.markSeen(chatId: chat.id)
                } label: {
                    Label("Read", systemImage: "checkmark")
                }
                .tint(Theme.statusCompleted.opacity(0.85))
            }
        }
    }

    private func sendBadge(_ state: SendState) -> (label: String, color: Color)? {
        switch state {
        case .failed: return ("Failed", Theme.danger)
        case .queued: return ("Queued", Theme.warning)
        case .sending: return nil
        }
    }

    private func content(indicator: ChatIndicator, sendState: SendState?,
                         reservesPullRequest: Bool) -> some View {
        let badge = sendState.flatMap(sendBadge)
        return VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 8) {
                if showLocation {
                    Text(location)
                        .font(Theme.sans(13))
                        .foregroundStyle(subline)
                        .lineLimit(1)
                        .truncationMode(.tail)
                        .frame(maxWidth: .infinity, alignment: .leading)
                } else {
                    Spacer(minLength: 4)
                }
                if let badge {
                    HStack(spacing: 4) {
                        Circle()
                            .fill(badge.color)
                            .frame(width: 6, height: 6)
                        Text(badge.label)
                            .font(Theme.sans(12, weight: .medium))
                            .foregroundStyle(badge.color)
                    }
                } else if indicator == .idle {
                    Text(relativeTime(chat.lastMessageAt ?? chat.createdAt))
                        .font(Theme.sans(12, weight: .medium))
                        .foregroundStyle(subline)
                        .fixedSize()
                } else {
                    StatusCorner(indicator: indicator)
                }
            }

            Text(chat.displayTitle)
                .font(Theme.sans(15, weight: .medium))
                .foregroundStyle(Theme.text)
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)

            if let preview = chat.lastMessagePreview?.trimmingCharacters(in: .whitespacesAndNewlines),
               !preview.isEmpty {
                Text(preview)
                    .font(Theme.sans(13))
                    .foregroundStyle(Theme.textMuted.opacity(0.72))
                    .lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }

            HStack(spacing: 4) {
                if let harness = chat.config?.harness {
                    HarnessBadge(harness: harness, size: 11, neutral: subline)
                }
                if let branch = chat.branch?.trimmingCharacters(in: .whitespaces), !branch.isEmpty {
                    LineIconView(.gitBranch, size: 11, color: subline)
                    Text(branch)
                        .font(Theme.sans(13))
                        .foregroundStyle(subline)
                        .lineLimit(1)
                        .truncationMode(.tail)
                }
                Spacer(minLength: 0)
                if indicator == .working, badge == nil {
                    MiniSpinner()
                }
            }
            .padding(.trailing, reservesPullRequest ? 46 : 0)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .contentShape(RoundedRectangle(cornerRadius: 8))
    }

    private var location: String {
        let space = chat.spaceId == nil ? "No project" : (
            model.space(for: chat)?.displayName
                ?? chat.cwd.map { ($0 as NSString).lastPathComponent }
                ?? "?"
        )
        return "\(space) @ \(model.deviceName(chat.deviceId))"
    }
}

func relativeTime(_ ms: Int64) -> String {
    let delta = max(0, nowMs() - ms) / 1000
    if delta < 60 { return "now" }
    if delta < 3600 { return "\(delta / 60)m" }
    if delta < 86_400 { return "\(delta / 3600)h" }
    return "\(delta / 86_400)d"
}
