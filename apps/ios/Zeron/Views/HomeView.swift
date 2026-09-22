// Home — the mobile shell. The desktop sidebar collapses into one screen: a
// space dropdown in the nav bar (default "All") scopes the attention-sorted
// session list below it. Tabs-as-sessions don't fit a phone; close=archive
// becomes swipe-to-archive.

import SwiftUI

enum Route: Hashable {
    case space(String)
    case chat(String)
    case newSession(NewSessionDestination)

    // Keep existing project navigation and launch/deep-link call sites valid.
    static func newSession(spaceId: String) -> Route {
        .newSession(.project(spaceId: spaceId))
    }
}

struct HomeView: View {
    @Environment(AppModel.self) private var model
    @State private var path: [Route] = []
    @State private var showNewSpace = false
    @State private var showProjectlessDevices = false
    @State private var showSettings = false
    @State private var confirmSignOut = false
    // "" = All. Sticky across launches; falls back to All if the space is gone.
    @AppStorage("homeSpaceFilter") private var spaceFilter: String = ""

    private var selectedSpace: Space? {
        model.spaces.first { $0.id == spaceFilter }
    }

    var body: some View {
        NavigationStack(path: $path) {
            SessionList(spaceId: selectedSpace?.id, path: $path) {
                if let selectedSpace { path.append(.newSession(spaceId: selectedSpace.id)) }
                else { showProjectlessDevices = true }
            }
            .navigationTitle("Cosmos")  // feeds the back menu; not displayed
            .navigationBarTitleDisplayMode(.inline)
            .toolbar(removing: .title)
            .navigationDestination(for: Route.self) { route in
                switch route {
                case .space(let id): SpaceView(spaceId: id, path: $path)
                case .chat(let id): SessionView(chatId: id)
                case .newSession(let destination): NewSessionView(destination: destination, path: $path)
                }
            }
            .toolbar {
                // Let the native toolbar own this glass surface and its menu
                // morph. A second custom glass layer retains stale masks.
                ToolbarItem(placement: .topBarLeading) {
                    HStack(spacing: 10) {
                        spaceDropdown
                        // In the bar, not the list: as a list row it appeared
                        // and vanished with the connection and shoved the
                        // content down. Degraded states are GRACED (4s of
                        // continuous raw degradation before anything shows;
                        // recovery hides instantly) and quiet — a bare
                        // grayscale spinner or dot with a faint caption, no
                        // surface, no border (shell.rs render_connection_pill).
                        switch model.connectivity.state {
                        case .offline:
                            HStack(spacing: 5) {
                                Circle()
                                    .fill(Theme.warning)
                                    .frame(width: 5, height: 5)
                                Text("Offline — sends are saved")
                                    .font(Theme.sans(13))
                                    .foregroundStyle(Theme.textFaint)
                            }
                            .transition(.opacity)
                        case .reconnecting:
                            HStack(spacing: 5) {
                                ProgressView()
                                    .controlSize(.mini)
                                    .tint(Theme.textMuted)
                                Text("Reconnecting…")
                                    .font(Theme.sans(13))
                                    .foregroundStyle(Theme.textFaint)
                            }
                            .transition(.opacity)
                        case .connected:
                            // Initial catch-up (within the grace): the old
                            // quiet "connecting" spinner, gone on first sync.
                            if !model.connected {
                                ProgressView()
                                    .controlSize(.mini)
                                    .tint(Theme.textMuted)
                                    .accessibilityLabel("Connecting")
                            }
                        }
                    }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    newButton
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Menu {
                        if model.demo != nil {
                            Text("Demo mode")
                        }
                        if (selectedSpace.map { model.chats(in: $0.id) } ?? model.overviewChats)
                            .contains(where: \.unseen) {
                            Button("Mark all as read", systemImage: "checkmark.circle") {
                                model.markAllSeen(in: selectedSpace?.id)
                            }
                        }
                        Button("Reconnect", systemImage: "arrow.clockwise") { model.foregrounded() }
                        // Sheet state must flip after the menu finishes
                        // dismissing, or the presentation is dropped.
                        Button("Settings", systemImage: "gear") {
                            DispatchQueue.main.async { showSettings = true }
                        }
                        .accessibilityIdentifier("open-settings")
                        Button("Sign out", role: .destructive) { confirmSignOut = true }
                    } label: {
                        Image(systemName: "person.circle")
                    }
                    .accessibilityLabel("Account")
                }
            }
            .confirmationDialog("Sign out of Cosmos?", isPresented: $confirmSignOut, titleVisibility: .visible) {
                Button("Sign out", role: .destructive) { model.signOut() }
            } message: {
                Text("Local session data will be removed from this phone. Your synced sessions remain on your account.")
            }
            .sheet(isPresented: $showNewSpace) {
                NewSpaceSheet { spaceId in
                    path.append(.space(spaceId))
                }
            }
            .sheet(isPresented: $showProjectlessDevices) {
                SessionHostPickerSheet { deviceId in
                    spaceFilter = ""
                    path.append(.newSession(.projectless(deviceId: deviceId)))
                }
            }
            .sheet(isPresented: $showSettings) {
                SettingsView()
            }
            .task(id: model.overviewChats.map(\.id).joined()) {
                model.preloadSessions()
            }
            .onAppear {
                if let route = model.launchRoute {
                    model.launchRoute = nil
                    // Push the whole stack atomically — appending from a child's
                    // onAppear mid-transition gets dropped by NavigationStack.
                    if case .space(let id) = route, model.launchSheet == "newsession" {
                        model.launchSheet = nil
                        path = [route, .newSession(spaceId: id)]
                    } else {
                        path = [route]
                    }
                }
                if model.launchSheet == "newspace" {
                    model.launchSheet = nil
                    showNewSpace = true
                }
            }
        }
        .opensWebLinksInApp()
    }

    // MARK: Space dropdown

    /// The nav-bar dropdown that scopes the session list — a NATIVE glass
    /// menu. Rows are Buttons, not a Picker: Picker menu rows drop two-Text
    /// subtitles, while Button rows map to UIAction subtitles, so each space
    /// shows its owning device ("@ mac") on the small second line without the
    /// three-line title wraps. Selection carries a checkmark in the icon slot.
    private var spaceDropdown: some View {
        Menu {
            spaceMenuButton(id: "", title: "All", subtitle: nil)
            ForEach(model.spaces) { space in
                spaceMenuButton(id: space.id, title: space.displayName,
                                subtitle: deviceTag(space))
            }
            Divider()
            Button {
                showNewSpace = true
            } label: {
                Label("New space…", systemImage: "folder.badge.plus")
            }
        } label: {
            HStack(spacing: 5) {
                Text(selectedSpace?.displayName ?? "All")
                    .font(Theme.sans(16, weight: .semibold))
                    .foregroundStyle(Theme.text)
                    .lineLimit(1)
                Image(systemName: "chevron.down")
                    .font(.system(size: 9, weight: .bold))
                    .foregroundStyle(Theme.textFaint)
            }
            // Keep long space names from swallowing the whole bar; the owning
            // device lives on the menu rows ("@ mac"), not up here.
            .frame(maxWidth: 200, alignment: .leading)
            .fixedSize(horizontal: true, vertical: false)
            .padding(.horizontal, 2)
            .frame(minHeight: 32)
        }
        .buttonStyle(.plain)
        .fixedSize(horizontal: true, vertical: false)
        .accessibilityLabel("Filter by space")
        .accessibilityIdentifier("space-filter")
    }

    private func deviceTag(_ space: Space) -> String {
        let name = model.deviceName(space.deviceId)
        return model.deviceOnline(space.deviceId) ? "@ \(name)" : "@ \(name) · offline"
    }

    private func spaceMenuButton(id: String, title: String, subtitle: String?) -> some View {
        let selected = id.isEmpty ? selectedSpace == nil : spaceFilter == id
        return Button {
            spaceFilter = id
        } label: {
            if selected {
                Label {
                    Text(title)
                    if let subtitle { Text(subtitle) }
                } icon: {
                    Image(systemName: "checkmark")
                }
            } else {
                Text(title)
                if let subtitle { Text(subtitle) }
            }
        }
    }

    /// Both destinations are available even when Home is scoped to a project
    /// or the workspace has no projects yet.
    private var newButton: some View {
        Menu {
            if let space = selectedSpace {
                Button("New session in \(space.displayName)") {
                    path.append(.newSession(spaceId: space.id))
                }
            } else if !model.spaces.isEmpty {
                Section("New session in…") {
                    ForEach(model.spaces) { space in
                        Button {
                            path.append(.newSession(spaceId: space.id))
                        } label: {
                            Text(space.displayName)
                            Text(deviceTag(space))
                        }
                    }
                }
            }
            Button {
                showProjectlessDevices = true
            } label: {
                Label("Session without a project…", systemImage: "xmark")
            }
            .accessibilityIdentifier("new-projectless-session")
            Button {
                showNewSpace = true
            } label: {
                Label("New space…", systemImage: "folder.badge.plus")
            }
        } label: {
            Image(systemName: "plus")
        }
        .accessibilityLabel("New session")
        .accessibilityIdentifier("new-session")
    }
}
