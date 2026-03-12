import * as spell from "cjs-lib";
import MyClass from "commonjs-pattern";

export const result = spell.spell("hello");
export const greeter = new MyClass("World");
export const greeting = greeter.greet();
