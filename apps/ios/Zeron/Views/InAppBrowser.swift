import Observation
import SwiftUI
import UIKit
import WebKit

struct BrowserDestination: Identifiable, Hashable {
    let url: URL
    var id: String { url.absoluteString }
}

/// Intercepts SwiftUI `openURL` (markdown links, PR badges) and presents the
/// in-app browser. Rejected destinations do not fall through to Safari.
private struct WebLinkRouter: ViewModifier {
    @State private var page: BrowserDestination?

    func body(content: Content) -> some View {
        content
            .environment(\.openURL, OpenURLAction { url in
                if let allowed = WebLink.transcriptURL(url) {
                    page = BrowserDestination(url: allowed)
                    return .handled
                }
                return .discarded
            })
            .sheet(item: $page) { destination in
                InAppBrowser(url: destination.url)
            }
    }
}

extension View {
    func opensWebLinksInApp() -> some View {
        modifier(WebLinkRouter())
    }
}

@Observable
final class InAppBrowserModel {
    let initialURL: URL
    var title: String
    var currentURL: URL
    var canGoBack = false
    var canGoForward = false
    var loading = false
    var error: String?
    @ObservationIgnored fileprivate weak var webView: WKWebView?

    init(url: URL) {
        initialURL = url
        currentURL = url
        title = url.host ?? "Browser"
    }

    func goBack() { webView?.goBack() }
    func goForward() { webView?.goForward() }
    func reload() {
        error = nil
        webView?.reload()
    }

    func openInSafari() {
        UIApplication.shared.open(currentURL)
    }
}

struct InAppBrowser: View {
    @Environment(\.dismiss) private var dismiss
    @State private var model: InAppBrowserModel

    init(url: URL) {
        _model = State(initialValue: InAppBrowserModel(url: url))
    }

    var body: some View {
        NavigationStack {
            ZStack {
                Theme.bg.ignoresSafeArea()
                CosmosWebView(model: model)
                if let error = model.error {
                    VStack(spacing: 12) {
                        Image(systemName: "wifi.exclamationmark")
                            .font(.system(size: 28, weight: .light))
                            .foregroundStyle(Theme.textMuted)
                        Text("Couldn't load page")
                            .font(Theme.sans(18, weight: .medium))
                        Text(error)
                            .font(Theme.sans(14))
                            .foregroundStyle(Theme.textMuted)
                            .multilineTextAlignment(.center)
                        Button("Try again", action: model.reload)
                            .frame(minHeight: 44)
                    }
                    .padding(28)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(Theme.bg)
                }
            }
            .navigationTitle(model.title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Done") { dismiss() }
                        .accessibilityIdentifier("close-in-app-browser")
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Menu {
                        Button("Open in Safari", systemImage: "safari", action: model.openInSafari)
                        ShareLink(item: model.currentURL)
                        Button("Copy Link", systemImage: "doc.on.doc") {
                            UIPasteboard.general.string = model.currentURL.absoluteString
                        }
                    } label: {
                        Image(systemName: "ellipsis.circle")
                    }
                    .accessibilityLabel("Page actions")
                }
                ToolbarItemGroup(placement: .bottomBar) {
                    Button(action: model.goBack) {
                        Image(systemName: "chevron.backward")
                    }
                    .disabled(!model.canGoBack)
                    .accessibilityLabel("Back")
                    Button(action: model.goForward) {
                        Image(systemName: "chevron.forward")
                    }
                    .disabled(!model.canGoForward)
                    .accessibilityLabel("Forward")
                    Spacer()
                    if model.loading {
                        ProgressView()
                            .controlSize(.mini)
                            .accessibilityLabel("Loading")
                    } else {
                        Button(action: model.reload) {
                            Image(systemName: "arrow.clockwise")
                        }
                        .accessibilityLabel("Reload")
                    }
                }
            }
        }
        .presentationBackground(Theme.bg)
        .presentationDetents([.large])
        .presentationDragIndicator(.visible)
        .ignoresSafeArea(.keyboard)
    }
}

private struct CosmosWebView: UIViewRepresentable {
    let model: InAppBrowserModel

    func makeCoordinator() -> Coordinator { Coordinator(model: model) }

    func makeUIView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        config.allowsInlineMediaPlayback = true
        config.defaultWebpagePreferences.preferredContentMode = .mobile
        let view = WKWebView(frame: .zero, configuration: config)
        view.allowsBackForwardNavigationGestures = true
        view.backgroundColor = UIColor(Theme.bg)
        view.scrollView.backgroundColor = UIColor(Theme.bg)
        view.isOpaque = false
        view.navigationDelegate = context.coordinator
        view.uiDelegate = context.coordinator
        model.webView = view
        view.load(URLRequest(url: model.initialURL))
        return view
    }

    func updateUIView(_ uiView: WKWebView, context: Context) {
        context.coordinator.model = model
        model.webView = uiView
    }

    final class Coordinator: NSObject, WKNavigationDelegate, WKUIDelegate {
        var model: InAppBrowserModel
        init(model: InAppBrowserModel) { self.model = model }

        func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction,
                     decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
            guard let url = navigationAction.request.url else {
                decisionHandler(.cancel)
                return
            }
            if url.absoluteString == "about:blank" || WebLink.allowsNavigation(url) {
                decisionHandler(.allow)
            } else {
                decisionHandler(.cancel)
            }
        }

        func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
            model.loading = true
            model.error = nil
            publish(webView)
        }

        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
            model.loading = false
            model.error = nil
            publish(webView)
        }

        func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
            fail(webView, error: error)
        }

        func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!,
                     withError error: Error) {
            fail(webView, error: error)
        }

        func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
                     for navigationAction: WKNavigationAction,
                     windowFeatures: WKWindowFeatures) -> WKWebView? {
            if navigationAction.targetFrame == nil,
               let url = navigationAction.request.url,
               WebLink.allowsNavigation(url) {
                webView.load(URLRequest(url: url))
            }
            return nil
        }

        private func fail(_ webView: WKWebView, error: Error) {
            let nsError = error as NSError
            if nsError.domain == NSURLErrorDomain, nsError.code == NSURLErrorCancelled { return }
            model.loading = false
            model.error = error.localizedDescription
            publish(webView)
        }

        private func publish(_ webView: WKWebView) {
            let trimmed = webView.title?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            model.title = trimmed.isEmpty ? (webView.url?.host ?? "Browser") : trimmed
            if let url = webView.url, WebLink.allowsNavigation(url) {
                model.currentURL = url
            }
            model.canGoBack = webView.canGoBack
            model.canGoForward = webView.canGoForward
        }
    }
}
