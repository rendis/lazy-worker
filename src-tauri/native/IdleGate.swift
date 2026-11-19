import Foundation

struct IdleGate {
    static let interval: TimeInterval = 45
    private var nextMovement: TimeInterval = 0

    func allowsMovement(idleSeconds: TimeInterval?, at uptime: TimeInterval) -> Bool {
        guard let idleSeconds, idleSeconds >= Self.interval else { return false }
        return uptime >= nextMovement
    }

    mutating func recordMovement(at uptime: TimeInterval) {
        nextMovement = uptime + Self.interval
    }
}
