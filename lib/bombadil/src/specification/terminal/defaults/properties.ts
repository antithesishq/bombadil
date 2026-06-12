import { not, always } from "@antithesishq/bombadil";
import { extract } from "@antithesishq/bombadil/terminal";

const exitStatus = extract((state) => state.exitStatus);

export const exitSuccess = always(
  not(
    () =>
      !!exitStatus.current &&
      exitStatus.current.signal == null &&
      exitStatus.current.code > 0,
  ),
);
