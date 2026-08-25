import { eventually } from "@antithesishq/bombadil";
import {
  actions,
  extract,
  registerCustomAction,
} from "@antithesishq/bombadil/browser";

const title = extract(
  (state) => state.document.querySelector("h1")?.textContent?.trim() ?? "",
);

export const hasReachedResultsPage = eventually(
  () => title.current === "Bombadil",
).within(5, "seconds");

const search = registerCustomAction(
  "search",
  async (document, _window, options: { query: string }) => {
    const searchButton = document.querySelector<HTMLButtonElement>(
      "#search-form [type=submit]",
    );
    if (!searchButton) {
      throw new Error("Search button not found");
    }
    const searchInput = document.querySelector<HTMLInputElement>(
      "#search-form [type=search]",
    );
    if (!searchInput) {
      throw new Error("Search input not found");
    }

    searchInput.focus();
    searchInput.value = options.query;
    searchInput.dispatchEvent(new Event("input", { bubbles: true }));

    searchButton.click();
  },
);

export const myActions = actions(() => [search({ query: "Bombadil" }), "Wait"]);
