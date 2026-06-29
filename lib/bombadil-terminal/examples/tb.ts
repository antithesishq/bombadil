import { ActionGenerator, branch, leaf } from "@antithesishq/bombadil/actions";
import { ActionTemplate } from "@antithesishq/bombadil/terminal";
export {
  exitSuccess,
  noReplacementChars,
} from "@antithesishq/bombadil/terminal/defaults";

export const actions = new ActionGenerator(() => {
  const regexps = [
    // "create_accounts id=1 code=10 ledger=700 flags=linked|history, id=2 code=10 ledger=700",
    // "create_transfers id=1 debit_account_id=1 credit_account_id=2 amount=1_000 ledger=700 code=10",
    // "lookup_accounts id=0xa1a2a3a4_b1b2_c1c2_d1d2_e1e2e3e4e5e6",
    // "lookup_transfers id=1, id=2",
    // "get_account_transfers timestamp_min=123 timestamp_max=456 account_id=1 flags=debits|credits",
    // "get_account_balances timestamp_min=123 timestamp_max=456 account_id=1 flags=debits|credits",
    // "query_accounts timestamp_min=123 timestamp_max=456",
    "query_transfers timestamp_min=123 timestamp_max=456",

    // "create_accounts",
    // "create_transfers",
    // "lookup_accounts",
    // "lookup_transfers",
    // "get_account_transfers",
    // "get_account_balances",
    // "query_accounts",
    // "query_transfers",
    // " = ",
    // "id",
    // "code",
    // "ledger",
    // "flags",
    // "timestamp_min",
    // "timestamp_max",
    // "linked",
    // "history",
  ];

  return branch([
    // [1, typeBasicInput.generate()],
    // [
    //   1,
    //   leaf({
    //     TypeText: { CharSet: CharSet.fromLiterals("\r\n") },
    //   } as ActionTemplate),
    // ],
    [
      1,
      branch(
        regexps.map((r) => [
          1,
          leaf({
            TypeText: { Regexp: r + "\n" },
          } as ActionTemplate),
        ]),
      ),
    ],
  ]);
});
