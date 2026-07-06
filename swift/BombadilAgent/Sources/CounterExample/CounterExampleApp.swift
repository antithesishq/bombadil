import BombadilAgent
import SwiftUI

/// A deliberately buggy counter: it is supposed to never go below
/// zero, but the decrement button doesn't check. Bombadil's example
/// specification (`examples/swiftui_counter.ts`) finds this.
@main
struct CounterExampleApp: App {
    init() {
        BombadilAgent.startIfRequested()
    }

    var body: some Scene {
        WindowGroup {
            CounterView()
        }
    }
}

struct CounterView: View {
    @State private var count = 0

    var body: some View {
        VStack(spacing: 20) {
            Text("\(count)")
                .font(.largeTitle)
                .accessibilityIdentifier("count")
                .accessibilityValue("\(count)")

            HStack(spacing: 20) {
                Button("Increment") {
                    count += 1
                }
                .accessibilityIdentifier("increment")

                Button("Decrement") {
                    // Bug: no lower bound.
                    count -= 1
                }
                .accessibilityIdentifier("decrement")
            }
        }
        .padding(40)
        .frame(minWidth: 320, minHeight: 200)
    }
}
