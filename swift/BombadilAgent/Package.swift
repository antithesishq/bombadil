// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "BombadilAgent",
    platforms: [
        .macOS(.v13)
    ],
    products: [
        .library(name: "BombadilAgent", targets: ["BombadilAgent"]),
        .executable(name: "CounterExample", targets: ["CounterExample"]),
    ],
    targets: [
        .target(name: "BombadilAgent"),
        .executableTarget(
            name: "CounterExample",
            dependencies: ["BombadilAgent"]
        ),
    ]
)
