import XCTest
@testable import SampleApp

final class OrderTests: XCTestCase {
    func testSave() {
        Order(id: 1).save()
    }
}
