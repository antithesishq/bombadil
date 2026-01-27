import { always, condition, eventually, extract, time } from "./bombadil";

// Invariant

const notification_count = extract(
  (state) => state.document.body.querySelectorAll(".notification").length,
);

export const max_notifications_shown = always(
  () => notification_count.current <= 5,
);

// Sliding window with .at()

export const constant_notification_count = condition(() => {
  const start = time.current;
  return always(
    () => notification_count.current === notification_count.at(start),
  );
});

// Temporal property

const error_message = extract(
  (state) => state.document.body.querySelector(".error")?.textContent ?? null,
);

export const error_disappears = always(
  condition(() => error_message !== null).implies(
    eventually(() => error_message === null).within(5, "seconds"),
  ),
);

export const error_disappears_no_implies = always(() =>
  error_message !== null
    ? eventually(() => error_message === null).within(5, "seconds")
    : true,
);

// "Contextful" temporal property, i.e. future predicates depend on past state

const name = extract((state) => {
  const element = state.document.body.querySelector("#name-field");
  return (element as HTMLInputElement | null)?.value ?? null;
});

const submit_in_progress = extract(
  (state) => state.document.body.querySelector("submit.progress") !== null,
);

const notification_text = extract(
  (state) =>
    state.document.body.querySelector(".notification")?.textContent ?? null,
);

export const contextful_notification_check = always(() => {
  const name_entered = name.current?.trim() ?? "";

  if (name_entered !== "" && submit_in_progress.current) {
    return eventually(
      () => notification_text?.current?.includes(name_entered) ?? false,
    ).within(5, "seconds");
  }

  return true;
});
