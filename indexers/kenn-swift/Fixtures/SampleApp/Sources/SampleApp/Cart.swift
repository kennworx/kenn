public struct Cart {
    public var orders: [Order] = []
    public init() {}
    public func checkout() {
        for order in orders {
            order.save()
        }
    }
}
