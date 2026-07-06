import { always, extract, not } from "@antithesishq/bombadil";
import { State } from "@antithesishq/bombadil/swiftui";

const exitStatus = extract<State, State["exitStatus"]>(
  (state) => state.exitStatus,
).named("exitStatus");

// The app must not crash: exiting with a signal or a non-zero exit
// code violates this property.
export const noCrash = always(
  not(
    () =>
      !!exitStatus.current &&
      (exitStatus.current.signal != null || exitStatus.current.code > 0),
  ),
);
