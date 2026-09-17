import SwiftUI

struct RenameSessionSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let chat: Chat
    @State private var title: String
    @FocusState private var focused: Bool

    init(chat: Chat) {
        self.chat = chat
        _title = State(initialValue: chat.displayTitle)
    }

    private var trimmed: String { title.trimmingCharacters(in: .whitespacesAndNewlines) }

    var body: some View {
        NavigationStack {
            Form {
                Section("Session name") {
                    TextField("Session name", text: $title, axis: .vertical)
                        .focused($focused)
                        .lineLimit(1...4)
                        .submitLabel(.done)
                        .onSubmit(save)
                        .accessibilityIdentifier("session-name")
                }
                .listRowBackground(SheetStyle.cardFill)
            }
            .scrollContentBackground(.hidden)
            .background(Theme.bg)
            .navigationTitle("Rename session")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save", action: save)
                        .disabled(trimmed.isEmpty)
                        .accessibilityIdentifier("save-session-name")
                }
            }
            .onAppear { focused = true }
        }
        .presentationDetents([.medium, .large])
    }

    private func save() {
        guard !trimmed.isEmpty else { return }
        model.rename(chatId: chat.id, title: trimmed)
        dismiss()
    }
}

/// Both active rows and the archived shelf expose the same management actions.
private struct SessionRowActions: ViewModifier {
    @Environment(AppModel.self) private var model
    let chat: Chat
    @State private var renaming = false

    func body(content: Content) -> some View {
        content
            .contextMenu {
                Button("Rename", systemImage: "pencil") { renaming = true }
                if chat.unseen {
                    Button("Mark as read", systemImage: "checkmark") { model.markSeen(chatId: chat.id) }
                }
                Button(chat.archived ? "Unarchive" : "Archive",
                       systemImage: chat.archived ? "arrow.up.bin" : "archivebox", action: archive)
            }
            .accessibilityAction(named: "Rename") { renaming = true }
            .accessibilityAction(named: Text(chat.archived ? "Unarchive" : "Archive")) { archive() }
            .sheet(isPresented: $renaming) { RenameSessionSheet(chat: chat) }
    }

    private func archive() {
        withAnimation(Motion.resort) {
            if chat.archived { model.unarchive(chatId: chat.id) }
            else { model.archive(chatId: chat.id) }
        }
    }
}

extension View {
    func sessionRowActions(chat: Chat) -> some View {
        modifier(SessionRowActions(chat: chat))
    }
}
