import { useEffect, useState, type ComponentType } from "react";
import type { Extension } from "@codemirror/state";
import { readDocumentTheme, type ResolvedTheme } from "../theme.js";

type Props = {
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
  required?: boolean;
  /** Approximate visible rows (maps to min-height). */
  rows?: number;
  placeholder?: string;
  className?: string;
  "data-testid"?: string;
};

type CodeMirrorProps = {
  value: string;
  height?: string;
  theme?: unknown;
  extensions?: Extension[];
  onChange?: (value: string) => void;
  editable?: boolean;
  basicSetup?: Record<string, boolean>;
  placeholder?: string;
};

type CmBundle = {
  CodeMirror: ComponentType<CodeMirrorProps>;
  yaml: () => Extension;
  chromeTheme: Extension;
  lineWrapping: Extension;
  editableOf: (v: boolean) => Extension;
  vscodeDark: unknown;
  vscodeLight: unknown;
};

/**
 * YAML editor with CodeMirror highlighting. Falls back to a plain textarea
 * until the editor chunk mounts (keeps `renderToString` UI tests working and
 * keeps CodeMirror out of the initial board bundle).
 */
export function YamlEditor({
  value,
  onChange,
  disabled = false,
  required = false,
  rows = 10,
  placeholder,
  className,
  "data-testid": testId,
}: Props) {
  const [cm, setCm] = useState<CmBundle | null>(null);
  const [theme, setTheme] = useState<ResolvedTheme>("light");

  useEffect(() => {
    let cancelled = false;
    setTheme(readDocumentTheme());
    const el = document.documentElement;
    const obs = new MutationObserver(() => setTheme(readDocumentTheme()));
    obs.observe(el, { attributes: true, attributeFilter: ["data-theme"] });

    void (async () => {
      const [
        { default: CodeMirror },
        { yaml },
        { EditorView },
        { vscodeDark, vscodeLight },
      ] = await Promise.all([
        import("@uiw/react-codemirror"),
        import("@codemirror/lang-yaml"),
        import("@codemirror/view"),
        import("@uiw/codemirror-theme-vscode"),
      ]);
      if (cancelled) return;
      const chromeTheme = EditorView.theme({
        "&": {
          border: "1px solid var(--line-strong)",
          borderRadius: "var(--radius-sm)",
          backgroundColor: "var(--panel-inset)",
        },
        "&.cm-focused": {
          outline:
            "2px solid color-mix(in srgb, var(--accent) 45%, transparent)",
          outlineOffset: "0",
        },
        ".cm-scroller": {
          fontFamily:
            "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
          fontSize: "12px",
          lineHeight: "1.4",
        },
        ".cm-gutters": {
          backgroundColor: "var(--panel-2)",
          color: "var(--dim)",
          borderRight: "1px solid var(--line)",
        },
        ".cm-activeLineGutter, .cm-activeLine": {
          backgroundColor:
            "color-mix(in srgb, var(--accent) 8%, transparent)",
        },
        ".cm-placeholder": {
          color: "var(--muted)",
        },
      });
      setCm({
        CodeMirror: CodeMirror as ComponentType<CodeMirrorProps>,
        yaml,
        chromeTheme,
        lineWrapping: EditorView.lineWrapping,
        editableOf: (v) => EditorView.editable.of(v),
        vscodeDark,
        vscodeLight,
      });
    })();

    return () => {
      cancelled = true;
      obs.disconnect();
    };
  }, []);

  const heightPx = Math.round(Math.max(8, rows) * 12 * 1.4 + 20);

  if (!cm) {
    return (
      <textarea
        className={className ?? "sandbox-policy-textarea"}
        value={value}
        disabled={disabled}
        required={required}
        rows={rows}
        spellCheck={false}
        placeholder={placeholder}
        data-testid={testId}
        onChange={(e) => onChange(e.target.value)}
      />
    );
  }

  const { CodeMirror, yaml, chromeTheme, lineWrapping, editableOf, vscodeDark, vscodeLight } =
    cm;

  return (
    <div
      className={["yaml-editor", className].filter(Boolean).join(" ")}
      data-testid={testId}
      data-yaml-editor="codemirror"
      aria-disabled={disabled || undefined}
    >
      <CodeMirror
        value={value}
        height={`${heightPx}px`}
        theme={theme === "dark" ? vscodeDark : vscodeLight}
        extensions={[
          yaml(),
          chromeTheme,
          lineWrapping,
          editableOf(!disabled),
        ]}
        onChange={onChange}
        editable={!disabled}
        basicSetup={{
          lineNumbers: true,
          foldGutter: true,
          highlightActiveLine: !disabled,
          bracketMatching: true,
          autocompletion: false,
        }}
        placeholder={placeholder}
      />
    </div>
  );
}
