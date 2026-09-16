// Auth client — Woozlit (Firebase) identity, matching the desktop engine.
// Desktop and iOS both bounce through woozlit.com/login; the returned
// custom token is exchanged for a Firebase ID token + refresh token via
// Google's Identity Toolkit. Dev mode (AUTH_MODE=dev edge) still uses
// bearer = user id ("user@org" for a fake org claim).

import Foundation

struct AuthUser: Codable, Equatable {
    var id: String
    var email: String?
    var firstName: String?
    var lastName: String?
}

struct AuthOrg: Codable, Identifiable, Equatable {
    var id: String
    var organizationId: String
    var name: String
}

struct AuthTokens: Codable, Equatable {
    var accessToken: String
    var refreshToken: String
}

enum AuthError: LocalizedError {
    case http(Int, String)
    case invalidResponse

    var errorDescription: String? {
        switch self {
        case .http(let code, let body): return "Auth failed (\(code)): \(body)"
        case .invalidResponse: return "Unexpected auth response"
        }
    }
}

struct AuthClient {
    var baseURL: URL

    static let firebaseApiKey = "AIzaSyBbJtkBoJrKM2hjW2H3o9pgtcyiGge9yOo"
    static let woozlitOrigin = URL(string: "https://www.woozlit.com")!

    /// Woozlit desktop/iOS bounce: custom token → Firebase ID + refresh tokens.
    func signInWithWoozlit(token: String, userId: String?, email: String?,
                           displayName: String?) async throws -> (AuthUser, AuthTokens) {
        if Self.isFirebaseIdToken(token) {
            let sub = Self.jwtSub(token) ?? userId ?? "woozlit-user"
            let user = AuthUser(id: userId ?? sub, email: email ?? Self.jwtEmail(token),
                                firstName: displayName, lastName: nil)
            return (user, AuthTokens(accessToken: token, refreshToken: ""))
        }
        struct Reply: Codable {
            var idToken: String
            var refreshToken: String
            var localId: String?
        }
        var request = URLRequest(url: URL(string:
            "https://identitytoolkit.googleapis.com/v1/accounts:signInWithCustomToken?key=\(Self.firebaseApiKey)")!)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONSerialization.data(withJSONObject: [
            "token": token,
            "returnSecureToken": true,
        ])
        let (data, response) = try await URLSession.shared.data(for: request)
        try Self.check(data: data, response: response)
        let reply = try JSONDecoder().decode(Reply.self, from: data)
        let id = userId ?? reply.localId ?? Self.jwtSub(reply.idToken) ?? "woozlit-user"
        let user = AuthUser(id: id,
                            email: email ?? Self.jwtEmail(reply.idToken),
                            firstName: displayName, lastName: nil)
        return (user, AuthTokens(accessToken: reply.idToken, refreshToken: reply.refreshToken))
    }

    func refreshWoozlit(refreshToken: String) async throws -> AuthTokens {
        var request = URLRequest(url: URL(string:
            "https://securetoken.googleapis.com/v1/token?key=\(Self.firebaseApiKey)")!)
        request.httpMethod = "POST"
        request.setValue("application/x-www-form-urlencoded", forHTTPHeaderField: "Content-Type")
        request.httpBody = "grant_type=refresh_token&refresh_token=\(refreshToken)"
            .data(using: .utf8)
        let (data, response) = try await URLSession.shared.data(for: request)
        try Self.check(data: data, response: response)
        struct Reply: Codable {
            var idToken: String?
            var accessToken: String?
            var refreshToken: String?
            enum CodingKeys: String, CodingKey {
                case idToken = "id_token"
                case accessToken = "access_token"
                case refreshToken = "refresh_token"
            }
        }
        let reply = try JSONDecoder().decode(Reply.self, from: data)
        guard let access = reply.idToken ?? reply.accessToken else {
            throw AuthError.invalidResponse
        }
        return AuthTokens(accessToken: access, refreshToken: reply.refreshToken ?? refreshToken)
    }

    func exchange(code: String) async throws -> (AuthUser, AuthTokens) {
        struct Response: Codable {
            var user: AuthUser
            var accessToken: String
            var refreshToken: String
        }
        let r: Response = try await post("auth/exchange", body: ["code": code])
        return (r.user, AuthTokens(accessToken: r.accessToken, refreshToken: r.refreshToken))
    }

    func refresh(refreshToken: String, organizationId: String? = nil) async throws -> AuthTokens {
        var body: [String: String] = ["refreshToken": refreshToken]
        if let organizationId { body["organizationId"] = organizationId }
        return try await post("auth/refresh", body: body)
    }

    private static func isFirebaseIdToken(_ token: String) -> Bool {
        jwtIss(token)?.contains("securetoken.google.com") == true
    }

    private static func jwtIss(_ token: String) -> String? { jwtClaim(token, "iss") }
    private static func jwtSub(_ token: String) -> String? { jwtClaim(token, "sub") }
    private static func jwtEmail(_ token: String) -> String? { jwtClaim(token, "email") }

    private static func jwtClaim(_ token: String, _ key: String) -> String? {
        let segments = token.split(separator: ".")
        guard segments.count >= 2 else { return nil }
        var base64 = String(segments[1]).replacingOccurrences(of: "-", with: "+")
            .replacingOccurrences(of: "_", with: "/")
        while base64.count % 4 != 0 { base64 += "=" }
        guard let data = Data(base64Encoded: base64),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            return nil
        }
        return obj[key] as? String
    }

    func orgs(accessToken: String) async throws -> [AuthOrg] {
        struct Response: Codable { var orgs: [AuthOrg] }
        var request = URLRequest(url: baseURL.appending(path: "auth/orgs"))
        request.setValue("Bearer \(accessToken)", forHTTPHeaderField: "Authorization")
        let (data, response) = try await URLSession.shared.data(for: request)
        try Self.check(data: data, response: response)
        return try JSONDecoder().decode(Response.self, from: data).orgs
    }

    private func post<T: Decodable>(_ path: String, body: [String: String]) async throws -> T {
        var request = URLRequest(url: baseURL.appending(path: path))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode(body)
        let (data, response) = try await URLSession.shared.data(for: request)
        try Self.check(data: data, response: response)
        return try JSONDecoder().decode(T.self, from: data)
    }

    private static func check(data: Data, response: URLResponse) throws {
        guard let http = response as? HTTPURLResponse else { throw AuthError.invalidResponse }
        guard (200..<300).contains(http.statusCode) else {
            throw AuthError.http(http.statusCode, String(data: data, encoding: .utf8) ?? "")
        }
    }
}

// MARK: - Keychain storage

enum Keychain {
    private static let service = "sh.zeron.ios"

    static func save(_ value: String, key: String) {
        let data = Data(value.utf8)
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key,
        ]
        SecItemDelete(query as CFDictionary)
        var add = query
        add[kSecValueData as String] = data
        add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
        SecItemAdd(add as CFDictionary, nil)
    }

    static func load(key: String) -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var result: AnyObject?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
              let data = result as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    static func delete(key: String) {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key,
        ]
        SecItemDelete(query as CFDictionary)
    }
}
