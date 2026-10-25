/**
 * Specification API for external drivers: drivers that run outside of
 * Bombadil and talk to it over the external driver protocol (for example
 * SwiftUI, Jetpack Compose or Flutter apps).
 *
 * The driver reports its state as arbitrary JSON, so specifications pick
 * their own state type `S` and extract from it. Actions are arbitrary JSON
 * too. Action templates are JSON in which some leaves are generators
 * ({@link integer}, {@link float}, {@link string}); Bombadil replaces each
 * generator with a concrete value before sending the action to the
 * driver.
 */
import {
  type ActionGenerator,
  type Cell,
  type JSON,
  type Tree,
} from "@antithesishq/bombadil";
import * as bombadil from "@antithesishq/bombadil";
import { Range, StringGenerator } from "@antithesishq/bombadil/actions";

/** A leaf in an {@link ActionTemplate} that Bombadil generates a value for. */
export type Generated =
  | { $generate: { Integer: Range } }
  | { $generate: { Float: Range } }
  | { $generate: { String: StringGenerator } };

/** JSON in which any value may be a {@link Generated} leaf. */
export type ActionTemplate =
  | string
  | number
  | boolean
  | null
  | Generated
  | ActionTemplate[]
  | { [key: string]: ActionTemplate };

/** A non-negative integer within `range`. */
export function integer(range: Range): Generated {
  return { $generate: { Integer: range } };
}

/** A non-negative number within `range`. */
export function float(range: Range): Generated {
  return { $generate: { Float: range } };
}

/** A string produced by `generator`. */
export function string(generator: StringGenerator): Generated {
  return { $generate: { String: generator } };
}

export function extract<S extends JSON, T extends JSON>(
  query: (state: S) => T,
): Cell<T> {
  return bombadil.extract<S, T>(query);
}

export function actions(
  generate: () => Tree<ActionTemplate> | ActionTemplate[],
): ActionGenerator<ActionTemplate> {
  return bombadil.actions<ActionTemplate>(generate);
}

export function weighted(
  value: [number, ActionTemplate | ActionGenerator<ActionTemplate>][],
): ActionGenerator<ActionTemplate> {
  return bombadil.weighted<ActionTemplate>(value);
}
