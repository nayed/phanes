import type { Element, ElementContent, Root as HastRoot } from "hast";
import type { Root as MdastRoot } from "mdast";
import type { VFile } from "vfile";
import { parse } from "yaml";

/**
 * Front matter (docs/PHANES.md, "Front matter"): the YAML block between `---`
 * lines at the top of a file, which static site generators and Obsidian use
 * for the title, date, tags… The preview shows it as a card of fields
 * instead of markdown (a rule and a jumbled heading).
 *
 * `remarkReadFrontMatter` (after remark-frontmatter found the block) reads it and
 * takes it out of the document; `rehypeFrontMatterCard` puts the card back at
 * the top after sanitizing. The card's markup is Phanes' own, and everything
 * from the file goes in as text, never as HTML.
 */

interface FrontMatter {
  /** The parsed YAML, or undefined when it isn't valid YAML. */
  data: unknown;
  /** The block's text, shown as is when it isn't valid YAML. */
  raw: string;
  /** Source line of the opening `---`, for synced scroll. */
  line: number;
  valid: boolean;
}

declare module "vfile" {
  interface DataMap {
    frontMatter?: FrontMatter;
  }
}

export function remarkReadFrontMatter() {
  return (tree: MdastRoot, file: VFile) => {
    const first = tree.children[0];
    if (first?.type !== "yaml") return;
    tree.children.shift();
    const raw = first.value;
    if (!raw.trim()) return; // `---` `---` with nothing between: no card
    const line = first.position?.start.line ?? 1;
    try {
      file.data.frontMatter = { data: parse(raw), raw, line, valid: true };
    } catch {
      file.data.frontMatter = { data: undefined, raw, line, valid: false };
    }
  };
}

export function rehypeFrontMatterCard() {
  return (tree: HastRoot, file: VFile) => {
    const fm = file.data.frontMatter;
    if (!fm) return;
    const body = isRecord(fm.data)
      ? Object.keys(fm.data).length
        ? fields(fm.data)
        : null
      : fm.valid && fm.data == null
        ? null
        : el("pre", ["front-matter-raw"], [text(fm.raw)]);
    if (!body) return;
    const card = el("section", ["front-matter"], [body], { dataLine: fm.line, ariaLabel: "Front matter" });
    tree.children.unshift(card);
  };
}

/* ---------- The card ---------- */

/** A list of fields: each key, muted, beside its value. */
function fields(record: Record<string, unknown>): Element {
  return el(
    "dl",
    ["front-matter-fields"],
    Object.entries(record).map(([key, value]) =>
      el("div", ["front-matter-field"], [el("dt", [], [text(key)]), el("dd", [], valueContent(value))]),
    ),
  );
}

/** How a value shows: lists of words as chips, nested fields as fields, yes/no for booleans. */
function valueContent(value: unknown): ElementContent[] {
  if (value === null || value === undefined || value === "") return [el("span", ["front-matter-empty"], [text("—")])];
  if (typeof value === "boolean") return [text(value ? "Yes" : "No")];
  if (value instanceof Date) return [text(isNaN(value.getTime()) ? String(value) : value.toISOString().slice(0, 10))];
  if (Array.isArray(value)) {
    if (value.length === 0) return valueContent(null);
    if (value.every(isScalar)) {
      return [el("ul", ["front-matter-chips"], value.map((item) => el("li", [], [text(scalarText(item))])))];
    }
    return [el("ul", ["front-matter-list"], value.map((item) => el("li", [], valueContent(item))))];
  }
  if (isRecord(value)) return Object.keys(value).length ? [fields(value)] : valueContent(null);
  return [text(String(value))];
}

const isScalar = (v: unknown) => v === null || ["string", "number", "boolean", "bigint"].includes(typeof v);
const scalarText = (v: unknown) => (typeof v === "boolean" ? (v ? "Yes" : "No") : v === null ? "—" : String(v));
const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v) && !(v instanceof Date);

function el(
  tagName: string,
  className: string[],
  children: ElementContent[],
  properties: Element["properties"] = {},
): Element {
  return {
    type: "element",
    tagName,
    properties: className.length ? { ...properties, className } : properties,
    children,
  };
}

function text(value: string): ElementContent {
  return { type: "text", value };
}
