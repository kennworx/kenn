open class Base {
    public init() {}
    open func run() {}
}

public final class Derived: Base {
    public override func run() {}
}
