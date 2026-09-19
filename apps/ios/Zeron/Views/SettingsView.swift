// Settings — a single native Form: appearance, connection, account, about.
// Presented as a sheet from Home's account menu.

import SwiftUI

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @AppStorage("cosmosAppearance") private var appearance = Appearance.system.rawValue
    @State private var confirmSignOut = false

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Picker("Appearance", selection: $appearance) {
                        ForEach(Appearance.allCases, id: \.rawValue) { option in
                            Text(option.title).tag(option.rawValue)
                        }
                    }
                    .pickerStyle(.segmented)
                } header: {
                    Text("Appearance")
                }

                Section("Connection") {
                    LabeledContent("Status") {
                        HStack(spacing: 6) {
                            Circle()
                                .fill(statusColor)
                                .frame(width: 8, height: 8)
                            Text(statusLabel)
                                .foregroundStyle(.secondary)
                        }
                    }
                    if model.demo == nil {
                        LabeledContent("Edge") {
                            Text(model.edgeURLString)
                                .font(Theme.mono(12))
                                .foregroundStyle(.secondary)
                                .lineLimit(1)
                                .truncationMode(.middle)
                        }
                    }
                    Button("Reconnect") {
                        model.foregrounded()
                    }
                    .accessibilityIdentifier("settings-reconnect")
                }

                Section("Account") {
                    if model.demo != nil {
                        LabeledContent("Mode") { Text("Demo").foregroundStyle(.secondary) }
                    } else {
                        LabeledContent("User") {
                            Text(model.storedUserId.isEmpty ? "—" : model.storedUserId)
                                .foregroundStyle(.secondary)
                                .lineLimit(1)
                                .truncationMode(.middle)
                        }
                        LabeledContent("Device") {
                            Text(model.deviceName)
                                .foregroundStyle(.secondary)
                        }
                    }
                    Button("Sign out", role: .destructive) {
                        confirmSignOut = true
                    }
                    .accessibilityIdentifier("settings-sign-out")
                }

                Section {
                    LabeledContent("Cosmos") {
                        Text(version)
                            .foregroundStyle(.secondary)
                            .monospacedDigit()
                    }
                } header: {
                    Text("About")
                } footer: {
                    Text("The companion app — drive remote Cosmos sessions from your phone.")
                }
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { dismiss() }
                        .accessibilityIdentifier("settings-done")
                }
            }
            .confirmationDialog("Sign out of Cosmos?", isPresented: $confirmSignOut,
                                titleVisibility: .visible) {
                Button("Sign out", role: .destructive) {
                    dismiss()
                    model.signOut()
                }
            } message: {
                Text("Local session data will be removed from this phone. Your synced sessions remain on your account.")
            }
        }
        .presentationDetents([.large])
        .presentationDragIndicator(.visible)
        // Presented sheets don't re-render when the root's scheme override
        // flips — apply it on the sheet content itself so the segmented
        // control visibly themes this sheet.
        .preferredColorScheme(Appearance(rawValue: appearance)?.scheme)
    }

    private var statusColor: Color {
        switch model.connectivity.state {
        case .offline: Theme.warning
        case .reconnecting: Theme.warning
        case .connected: Theme.statusCompleted
        }
    }

    private var statusLabel: String {
        switch model.connectivity.state {
        case .offline: "Offline — sends are saved"
        case .reconnecting: "Reconnecting…"
        case .connected: "Connected"
        }
    }

    private var version: String {
        let info = Bundle.main.infoDictionary
        let short = info?["CFBundleShortVersionString"] as? String ?? "?"
        let build = info?["CFBundleVersion"] as? String ?? "?"
        return "\(short) (\(build))"
    }
}
