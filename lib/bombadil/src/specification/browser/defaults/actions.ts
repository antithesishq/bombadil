import type { Cell } from "@antithesishq/bombadil";
import { emails, integers, keycodes, strings } from "@antithesishq/bombadil";
import {
	type Action,
	actions,
	extract,
	weighted,
} from "@antithesishq/bombadil/browser";

// ---------- Page-level state ----------

const contentType = extract((state) => state.document.contentType);

const canGoBack = extract((state) => state.navigationHistory.back.length > 0);

const canGoForwardSameOrigin = extract((state) => {
	const entry = state.navigationHistory.forward[0];
	if (!entry) return false;
	try {
		const current = new URL(state.navigationHistory.current.url);
		const forward = new URL(entry.url);
		return forward.origin === current.origin;
	} catch {
		return false;
	}
});

export const lastAction: Cell<Action | null> = extract(
	(state) => state.lastAction,
);

const body = extract((state) => {
	return state.document.body
		? { scrollHeight: state.document.body.scrollHeight }
		: null;
});

const window = extract((state) => {
	return {
		scroll: {
			x: state.window.scrollX,
			y: state.window.scrollY,
		},
		inner: {
			width: state.window.innerWidth,
			height: state.window.innerHeight,
		},
	};
});

export const waitOnce = actions(() => {
	if (lastAction.current !== "Wait") {
		return ["Wait"];
	} else {
		return [];
	}
});

export const scroll = actions(() => {
	if (contentType.current !== "text/html") return [];

	if (!body.current) return [];

	const scrollYMax = body.current.scrollHeight - window.current.inner.height;
	const scrollYMaxDiff = scrollYMax - window.current.scroll.y;

	if (scrollYMaxDiff >= 1) {
		return [
			{
				ScrollDown: {
					origin: {
						x: window.current.inner.width / 2,
						y: window.current.inner.height / 2,
					},
					distance: Math.min(window.current.inner.height / 2, scrollYMaxDiff),
				},
			} as Action,
		];
	} else if (window.current.scroll.y > 0) {
		return [
			{
				ScrollUp: {
					origin: {
						x: window.current.inner.width / 2,
						y: window.current.inner.height / 2,
					},
					distance: window.current.scroll.y,
				},
			} as Action,
		];
	}

	return [];
});

// ---------- Element scanning helpers ----------

type Point = { x: number; y: number };

function clickablePoint(element: Element): Point | null {
	const rect = element.getBoundingClientRect();
	if (rect.width > 0 && rect.height > 0) {
		return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
	}
	return null;
}

function isVisible(element: Element, domWindow: Window): boolean {
	const style = domWindow.getComputedStyle(element);
	return (
		style.display !== "none" &&
		style.visibility !== "hidden" &&
		parseFloat(style.opacity || "1") > 0.0
	);
}

function inViewport(point: Point, domWindow: Window): boolean {
	return (
		point.x >= 0 &&
		point.x <= domWindow.innerWidth &&
		point.y >= 0 &&
		point.y <= domWindow.innerHeight
	);
}

// Like querySelectorAll, but descends into shadow roots and same-origin iframes.
//
// TODO: make this a part of the bombadil package so that others can use it
// (depends on https://github.com/antithesishq/bombadil/issues/17)
function queryAll(root: Element, selector: string): Element[] {
	const queue: Element[] = [root];
	const results: Element[] = [];
	while (queue.length > 0) {
		const element = queue.pop()!;
		if (element.matches(selector)) {
			results.push(element);
		}
		if (element.shadowRoot) {
			for (const child of Array.from(element.shadowRoot.children)) {
				queue.push(child);
			}
		} else if (
			element instanceof HTMLIFrameElement &&
			element.contentDocument &&
			element.contentDocument.body
		) {
			queue.push(element.contentDocument.body);
		} else {
			for (const child of Array.from(element.children)) {
				queue.push(child);
			}
		}
	}
	return results;
}

function normalizeText(text: string | null | undefined): string {
	return (text ?? "").trim().replace(/\s+/g, " ");
}

function truncate(text: string, maxLength = 80): string {
	return text.length > maxLength ? `${text.slice(0, maxLength - 1)}…` : text;
}

function findLabel(
	element: HTMLInputElement | HTMLTextAreaElement,
): HTMLLabelElement | null {
	if (element.id) {
		const escaped = CSS.escape(element.id);
		const byFor = element.ownerDocument.querySelector(
			`label[for="${escaped}"]`,
		);
		if (byFor instanceof HTMLLabelElement) return byFor;
	}
	return element.closest("label");
}

function labelText(
	element: HTMLInputElement | HTMLTextAreaElement,
): string | null {
	const ariaLabel = element.getAttribute("aria-label");
	if (ariaLabel) return truncate(ariaLabel.trim());

	const labelledBy = element.getAttribute("aria-labelledby");
	if (labelledBy) {
		const referent = element.ownerDocument.getElementById(labelledBy);
		if (referent) return truncate(normalizeText(referent.textContent));
	}

	const label = findLabel(element);
	if (label) return truncate(normalizeText(label.textContent));

	return null;
}

// Locates a clickable point for a form control, falling back to its
// associated label when the control itself has no usable point. Common for
// inputs that are visually hidden and overlayed with custom styling.
function formControlPoint(
	element: HTMLInputElement | HTMLTextAreaElement,
	domWindow: Window,
): Point | null {
	const direct = clickablePoint(element);
	if (direct && inViewport(direct, domWindow)) return direct;

	const label = findLabel(element);
	if (label && isVisible(label, domWindow)) {
		const labelPoint = clickablePoint(label);
		if (labelPoint && inViewport(labelPoint, domWindow)) return labelPoint;
	}

	return null;
}

// ---------- Element snapshots ----------

const links = extract((state) => {
	if (!state.document.body) return [];

	let urlCurrent: URL;
	try {
		urlCurrent = new URL(state.window.location.toString());
	} catch {
		return [];
	}

	const result: {
		tag: string;
		text: string;
		href: string;
		point: Point;
	}[] = [];

	for (const anchor of queryAll(state.document.body, "a")) {
		if (!(anchor instanceof HTMLAnchorElement)) continue;

		let url: URL;
		try {
			url = new URL(anchor.href);
		} catch {
			continue;
		}

		if (anchor.target === "_blank") continue;
		if (!url.protocol.startsWith("http")) continue;
		if (url.hostname !== urlCurrent.hostname) continue;
		if (url.port !== "" && url.port !== urlCurrent.port) continue;
		if (!isVisible(anchor, state.window)) continue;

		const point = clickablePoint(anchor);
		if (!point) continue;
		if (!inViewport(point, state.window)) continue;

		result.push({
			tag: anchor.nodeName,
			text: truncate(normalizeText(anchor.textContent)),
			href: truncate(anchor.href, 120),
			point,
		});
	}
	return result;
});

const buttons = extract((state) => {
	if (!state.document.body) return [];

	const selector =
		"button:not(:disabled)," +
		"input[type=submit]:not(:disabled)," +
		"input[type=button]:not(:disabled)," +
		"input[type=reset]:not(:disabled)";

	const result: {
		tag: string;
		text: string;
		type: string;
		point: Point;
	}[] = [];

	for (const element of queryAll(state.document.body, selector)) {
		if (!isVisible(element, state.window)) continue;
		const point = clickablePoint(element);
		if (!point) continue;
		if (!inViewport(point, state.window)) continue;

		let text: string;
		let type: string;
		if (element instanceof HTMLButtonElement) {
			text = truncate(
				normalizeText(
					element.textContent || element.getAttribute("aria-label") || "",
				),
			);
			type = element.type || "submit";
		} else if (element instanceof HTMLInputElement) {
			text = truncate(
				normalizeText(
					element.value || element.getAttribute("aria-label") || element.type,
				),
			);
			type = element.type;
		} else {
			continue;
		}

		result.push({ tag: element.nodeName, text, type, point });
	}
	return result;
});

const inputElements = extract((state) => {
	if (!state.document.body) return [];

	type InputEntry = {
		tag: string;
		inputType: string;
		name: string | null;
		id: string | null;
		label: string | null;
		placeholder: string | null;
		value: string;
		checked?: boolean;
		required: boolean;
		focused: boolean;
		point: Point;
	};
	const result: InputEntry[] = [];

	for (const element of queryAll(state.document.body, "input:not(:disabled)")) {
		if (!(element instanceof HTMLInputElement)) continue;
		const inputType = element.type;
		if (
			inputType === "file" ||
			inputType === "submit" ||
			inputType === "button" ||
			inputType === "reset" ||
			inputType === "image" ||
			inputType === "hidden"
		) {
			continue;
		}

		// Inputs are often hidden behind custom-styled wrappers — fall back to
		// the label's point rather than requiring the input itself to be visible.
		const point = formControlPoint(element, state.window);
		if (!point) continue;

		const isCheckable = inputType === "checkbox" || inputType === "radio";
		const entry: InputEntry = {
			tag: element.nodeName,
			inputType,
			name: element.getAttribute("name"),
			id: element.id || null,
			label: labelText(element),
			placeholder: element.placeholder || null,
			value: truncate(element.value || ""),
			required: element.required,
			focused: element === state.document.activeElement,
			point,
		};
		if (isCheckable) entry.checked = element.checked;
		result.push(entry);
	}
	return result;
});

const textareas = extract((state) => {
	if (!state.document.body) return [];

	const result: {
		tag: string;
		name: string | null;
		id: string | null;
		label: string | null;
		placeholder: string | null;
		value: string;
		required: boolean;
		focused: boolean;
		point: Point;
	}[] = [];

	for (const element of queryAll(
		state.document.body,
		"textarea:not(:disabled)",
	)) {
		if (!(element instanceof HTMLTextAreaElement)) continue;
		if (!isVisible(element, state.window)) continue;

		const point = clickablePoint(element);
		if (!point) continue;
		if (!inViewport(point, state.window)) continue;

		result.push({
			tag: element.nodeName,
			name: element.getAttribute("name"),
			id: element.id || null,
			label: labelText(element),
			placeholder: element.placeholder || null,
			value: truncate(element.value || ""),
			required: element.required,
			focused: element === state.document.activeElement,
			point,
		});
	}
	return result;
});

const ariaClickables = extract((state) => {
	if (!state.document.body) return [];

	const roles = [
		"button",
		"link",
		"checkbox",
		"radio",
		"switch",
		"tab",
		"menuitem",
		"option",
		"treeitem",
	];
	const selector = roles.map((role) => `[role=${role}]`).join(",");

	const result: {
		tag: string;
		role: string;
		text: string;
		point: Point;
	}[] = [];

	for (const element of queryAll(state.document.body, selector)) {
		// Skip elements already covered by the typed snapshots above so we
		// don't emit duplicate click actions for the same DOM element.
		if (
			element instanceof HTMLAnchorElement ||
			element instanceof HTMLButtonElement ||
			element instanceof HTMLInputElement ||
			element instanceof HTMLTextAreaElement
		) {
			continue;
		}
		if (!isVisible(element, state.window)) continue;

		const point = clickablePoint(element);
		if (!point) continue;
		if (!inViewport(point, state.window)) continue;

		result.push({
			tag: element.nodeName,
			role: element.getAttribute("role") || "",
			text: truncate(normalizeText(element.textContent)),
			point,
		});
	}
	return result;
});

// ---------- Active input ----------

const activeInput = extract((state) => {
	const element = state.document.activeElement;
	if (!element || element === state.document.body) return null;

	if (element instanceof HTMLTextAreaElement) {
		return "textarea";
	}

	if (element instanceof HTMLInputElement) {
		return element.type;
	}

	return null;
});

// ---------- Action generators ----------

export const clicks = actions(() => {
	if (contentType.current !== "text/html") return [];

	const result: Action[] = [];

	for (const link of links.current) {
		result.push({
			Click: { name: link.tag, content: link.text, point: link.point },
		} as Action);
	}
	for (const button of buttons.current) {
		result.push({
			Click: { name: button.tag, content: button.text, point: button.point },
		} as Action);
	}
	for (const inputElement of inputElements.current) {
		// Avoid re-clicking an input that's already focused and has a value;
		// the model/random-walk should either type more or move on.
		if (inputElement.focused && inputElement.value) continue;
		result.push({
			Click: {
				name: inputElement.tag,
				content: "",
				point: inputElement.point,
			},
		} as Action);
	}
	for (const textarea of textareas.current) {
		if (textarea.focused && textarea.value) continue;
		result.push({
			Click: { name: textarea.tag, content: "", point: textarea.point },
		} as Action);
	}
	for (const ariaClickable of ariaClickables.current) {
		result.push({
			Click: {
				name: ariaClickable.tag,
				content: ariaClickable.text,
				point: ariaClickable.point,
			},
		} as Action);
	}
	return result;
});

export const typing = actions(() => {
	if (contentType.current !== "text/html") return [];
	const type = activeInput.current;
	if (!type) return [];

	if (type === "file") return [];

	const delayMillis = integers().min(1).max(100).generate();

	if (type === "textarea") {
		return weighted([
			[1, { PressKey: { code: keycodes().generate() } }],
			[3, { TypeText: { text: strings().minSize(1).generate(), delayMillis } }],
		]).generate();
	}

	switch (type) {
		case "text":
		case "password":
		case "search":
		case "tel":
		case "url":
			return weighted([
				[1, { PressKey: { code: keycodes().generate() } }],
				[
					3,
					{ TypeText: { text: strings().minSize(1).generate(), delayMillis } },
				],
			]).generate();
		case "email":
			return weighted([
				[1, { PressKey: { code: keycodes().generate() } }],
				[3, { TypeText: { text: emails().generate(), delayMillis } }],
			]).generate();
		case "number":
			return weighted([
				[1, { PressKey: { code: keycodes().generate() } }],
				[
					3,
					{
						TypeText: {
							text: integers().min(0).max(10000).generate().toString(),
							delayMillis,
						},
					},
				],
			]).generate();
		default:
			return [];
	}
});

// Navigation

export const back = actions(() => {
	if (canGoBack.current) return ["Back" as Action];
	return [];
});

export const forward = actions(() => {
	if (canGoForwardSameOrigin.current) return ["Forward" as Action];
	return [];
});

export const reload = actions(() => {
	if (lastAction.current !== "Reload" && lastAction.current !== "Wait")
		return ["Reload" as Action];
	return [];
});

export const navigation = weighted([
	[10, back],
	[1, forward],
	[1, reload],
]);

/**
 * @deprecated Renamed to {@link typing}. Will be removed in a future release.
 */
export const inputs = typing;
