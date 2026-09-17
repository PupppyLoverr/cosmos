// Space detail — the phone's answer to the desktop's horizontal session tabs:
// the space's sessions as a vertical list (creation order, like tab order),
// swipe-to-archive (= tab close), and "+" to start a session in this space.

import SwiftUI

struct SpaceView: View {
    @Environment(AppModel.self) private var model
    let spaceId: String
    @Binding var path: [Route]

    private var space: Space? {
        model.spaces.first { $0.id == spaceId }
    }

    var body: some View {
        SessionList(spaceId: spaceId, path: $path) {
            path.append(.newSession(spaceId: spaceId))
        }
        .navigationTitle(space?.displayName ?? "Space")  // feeds the back menu
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .principal) {
                VStack(spacing: 1) {
                    Text(space?.displayName ?? "Space")
                        .font(Theme.sans(13, weight: .medium))
                        .foregroundStyle(Theme.text)
                        .lineLimit(1)
                    if let space {
                        HStack(spacing: 4) {
                            Image(systemName: "folder")
                                .font(.system(size: 9))
                            Text("\(space.path) · \(model.deviceName(space.deviceId))")
                                .lineLimit(1)
                                .truncationMode(.head)
                        }
                        .font(Theme.sans(10.5))
                        .foregroundStyle(Theme.textMuted.opacity(0.6))
                    }
                }
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button {
                    path.append(.newSession(spaceId: spaceId))
                } label: {
                    Image(systemName: "plus")
                }
                .accessibilityLabel("New session")
            }
        }
        .onAppear {
            if model.launchSheet == "newsession" {
                model.launchSheet = nil
                path.append(.newSession(spaceId: spaceId))
            }
        }
    }
}
