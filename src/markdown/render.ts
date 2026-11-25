import rehypeRaw from "rehype-raw";
import rehypeSanitize from "rehype-sanitize";
import rehypeStringify from "rehype-stringify";
import remarkFrontmatter from "remark-frontmatter";
import remarkGfm from "remark-gfm";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import { unified, type PluggableList } from "unified";
import { rehypeAlerts } from "./plugins/alerts";
import { rehypeFrontMatterCard, remarkReadFrontMatter } from "./plugins/frontMatter";
import { rehypeShiki } from "./plugins/highlight";
import { rehypeLocalImages } from "./plugins/localImages";
import { sanitizeSchema } from "./plugins/schema";
import { rehypeSourceLines } from "./plugins/sourceLines";

/**
 * The markdown → HTML pipeline, in order. Adding a feature (math, Mermaid…)
 * means adding a plugin to this list:
 *
 * - remark plugins work on the markdown tree (before `remarkRehype`);
 * - rehype plugins before `rehypeSanitize` see untrusted HTML;
 * - rehype plugins after it only ever see sanitized HTML, and may add
 *   markup of their own (alerts, highlighting).
 */
export const pipeline: PluggableList = [
  remarkFrontmatter, // a YAML block between `---` lines at the top…
  remarkReadFrontMatter, // …read, and taken out of the document
  remarkGfm, // tables, task lists, strikethrough, autolinks, footnotes
  [remarkRehype, { allowDangerousHtml: true, clobberPrefix: "" }],
  rehypeRaw, // parse raw HTML in the document, so it can be sanitized like GitHub does
  rehypeSourceLines, // data-line on blocks, for synced scroll
  [rehypeSanitize, sanitizeSchema], // GitHub's schema: no scripts, ever
  rehypeFrontMatterCard, // the front matter as a card of fields, at the top
  rehypeAlerts, // > [!NOTE] …
  rehypeShiki, // code blocks
  rehypeLocalImages, // relative images through the asset protocol
];

const processor = unified().use(remarkParse).use(pipeline).use(rehypeStringify).freeze();

export interface RenderOptions {
  /** Folder of the document, for relative images. */
  baseDir?: string;
}

/** Render markdown to sanitized HTML. */
export async function renderMarkdown(markdown: string, options: RenderOptions = {}): Promise<string> {
  const file = await processor.process({ value: markdown, data: { baseDir: options.baseDir } });
  return String(file);
}
