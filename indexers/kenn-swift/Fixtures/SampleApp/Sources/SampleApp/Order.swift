public protocol Persistable {
    func save()
}

public struct Order: Persistable {
    public let id: Int
    public init(id: Int) { self.id = id }
    public func save() {}
    public func total() -> Int { id }
}
