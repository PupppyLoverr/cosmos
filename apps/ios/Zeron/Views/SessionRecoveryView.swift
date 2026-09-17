import SwiftUI

/// Slow sync keeps an honest loading state, but never strands the user behind
/// an endless skeleton. Cancellation prevents a departed screen changing state.
struct SessionRecoveryView: View {
    let title: String
    let detail: String
    let retry: () -> Void
    var close: (() -> Void)? = nil
    @State private var showRecovery = false
    @State private var attempt = 0

    var body: some View {
        VStack(spacing: 16) {
            if showRecovery {
                Image(systemName: "arrow.triangle.2.circlepath")
                    .font(.system(size: 28, weight: .light))
                    .foregroundStyle(Theme.textMuted)
                Text("Taking longer than usual")
                    .font(Theme.sans(18, weight: .medium))
                Text(detail)
                    .font(Theme.sans(14))
                    .foregroundStyle(Theme.textMuted)
                    .multilineTextAlignment(.center)
                Button("Try again") {
                    showRecovery = false
                    attempt += 1
                    retry()
                }
                .frame(minHeight: 44)
                .accessibilityIdentifier("retry-session-sync")
                if let close {
                    Button("Back to sessions", action: close)
                        .frame(minHeight: 44)
                }
            } else {
                CosmosPulse()
                Text(title)
                    .font(Theme.sans(14))
                    .foregroundStyle(Theme.textMuted)
            }
        }
        .padding(28)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.bg)
        .task(id: attempt) {
            do {
                try await Task.sleep(for: .seconds(10))
                showRecovery = true
            } catch { /* View disappeared or retry superseded this wait. */ }
        }
    }
}
