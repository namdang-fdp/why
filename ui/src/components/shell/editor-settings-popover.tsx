import { useState } from "react";
import { Settings2 } from "lucide-react";
import { useStore } from "zustand";
import { Button } from "@/components/ui/button";
import { PopoverContent, PopoverRoot, PopoverTrigger, Tip } from "@/components/ui/overlays";
import {
  defaultEditorSettings,
  editorSettingsStore,
  fontSizeRange,
  lineHeightRange,
  themeLabels,
  whitespaceLabels,
  type EditorTheme,
  type WhitespaceMode,
} from "@/stores/editor-settings";
import { useWorkspace } from "@/stores/workspace-context";

/** A text field that applies its value when the user leaves it or presses Enter. */
function FontFamilyField({
  value,
  onCommit,
}: {
  value: string;
  onCommit: (value: string) => void;
}) {
  const [draft, setDraft] = useState(value);
  const [synced, setSynced] = useState(value);
  // Follow the stored value when it changes elsewhere (Reset).
  if (value !== synced) {
    setSynced(value);
    setDraft(value);
  }
  return (
    <input
      id="editor-font-family"
      className="input mono"
      value={draft}
      spellCheck={false}
      autoComplete="off"
      onChange={(event) => setDraft(event.target.value)}
      onBlur={() => onCommit(draft)}
      onKeyDown={(event) => {
        if (event.key === "Enter") onCommit(draft);
      }}
    />
  );
}

/**
 * A number field you can type in. It keeps its own text while you edit (so
 * clearing it to type a new number works), applies every valid number at once
 * (the store clamps it to the range) and shows the stored value again on blur.
 */
function NumberField({
  id,
  value,
  range,
  onCommit,
}: {
  id: string;
  value: number;
  range: { min: number; max: number };
  onCommit: (value: number) => void;
}) {
  const [draft, setDraft] = useState(String(value));
  const [synced, setSynced] = useState(value);
  const [editing, setEditing] = useState(false);
  // Follow the stored value when it changes elsewhere (Reset, the palette), but
  // never while typing: the store clamps "1" to 10, and rewriting the field then
  // would turn the next keystroke into "105".
  if (value !== synced) {
    setSynced(value);
    if (!editing) setDraft(String(value));
  }
  return (
    <input
      id={id}
      className="input mono"
      type="number"
      min={range.min}
      max={range.max}
      value={draft}
      onChange={(event) => {
        setDraft(event.target.value);
        const parsed = event.target.valueAsNumber;
        if (Number.isFinite(parsed)) onCommit(parsed);
      }}
      onFocus={() => setEditing(true)}
      onBlur={() => {
        setEditing(false);
        setDraft(String(value));
      }}
    />
  );
}

function EditorSettingsForm() {
  const settings = useStore(editorSettingsStore, (state) => state.settings);
  const update = useStore(editorSettingsStore, (state) => state.update);
  const reset = useStore(editorSettingsStore, (state) => state.reset);

  return (
    <div className="settings-form">
      <h2 className="settings-title">Editor</h2>

      <div className="setting">
        <label htmlFor="editor-font-family">Font</label>
        <FontFamilyField
          value={settings.fontFamily}
          onCommit={(fontFamily) => update({ fontFamily })}
        />
        <p className="field-hint">The first installed face in the list is used.</p>
      </div>

      <div className="setting-pair">
        <div className="setting">
          <label htmlFor="editor-font-size">Size</label>
          <NumberField
            id="editor-font-size"
            value={settings.fontSize}
            range={fontSizeRange}
            onCommit={(fontSize) => update({ fontSize })}
          />
        </div>
        <div className="setting">
          <label htmlFor="editor-line-height">Line height</label>
          <NumberField
            id="editor-line-height"
            value={settings.lineHeight}
            range={lineHeightRange}
            onCommit={(lineHeight) => update({ lineHeight })}
          />
        </div>
      </div>

      <div className="setting">
        <label htmlFor="editor-theme">Theme</label>
        <select
          id="editor-theme"
          className="input"
          value={settings.theme}
          onChange={(event) => update({ theme: event.target.value as EditorTheme })}
        >
          {Object.entries(themeLabels).map(([value, label]) => (
            <option key={value} value={value}>
              {label}
            </option>
          ))}
        </select>
      </div>

      <div className="setting">
        <label htmlFor="editor-whitespace">Show whitespace</label>
        <select
          id="editor-whitespace"
          className="input"
          value={settings.whitespace}
          onChange={(event) => update({ whitespace: event.target.value as WhitespaceMode })}
        >
          {Object.entries(whitespaceLabels).map(([value, label]) => (
            <option key={value} value={value}>
              {label}
            </option>
          ))}
        </select>
      </div>

      <label className="setting-check">
        <input
          type="checkbox"
          checked={settings.ligatures}
          onChange={(event) => update({ ligatures: event.target.checked })}
        />
        <span>Font ligatures</span>
      </label>

      <div className="modal-actions">
        <Button
          type="button"
          variant="ghost"
          size="sm"
          disabled={JSON.stringify(settings) === JSON.stringify(defaultEditorSettings)}
          onClick={reset}
        >
          Reset to defaults
        </Button>
      </div>
    </div>
  );
}

/** The gear in the top bar. Open state lives in the workspace store so the palette can open it too. */
export function EditorSettingsPopover() {
  const open = useWorkspace((state) => state.editorSettingsOpen);
  const setOpen = useWorkspace((state) => state.setEditorSettingsOpen);
  return (
    <PopoverRoot open={open} onOpenChange={setOpen}>
      <Tip label="Editor settings" side="bottom">
        <PopoverTrigger asChild>
          <Button
            id="shell-editor-settings"
            aria-label="Editor settings"
            variant={open ? "secondary" : "ghost"}
            size="icon-sm"
          >
            <Settings2 aria-hidden />
          </Button>
        </PopoverTrigger>
      </Tip>
      <PopoverContent
        align="end"
        className="settings-popover"
        // Opened from the palette, focus is still being handed back by the closing
        // palette; that must not count as "the user clicked away".
        onFocusOutside={(event) => event.preventDefault()}
      >
        <EditorSettingsForm />
      </PopoverContent>
    </PopoverRoot>
  );
}
