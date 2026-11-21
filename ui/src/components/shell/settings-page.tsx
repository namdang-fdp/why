import { useLayoutEffect, useRef } from "react";
import { ArrowLeft, Plug, Type } from "lucide-react";
import { AgentHooksSection } from "@/components/hooks/agent-hooks-section";
import type { SettingsSection } from "@/stores/workspace";
import { useWorkspace } from "@/stores/workspace-context";
import { EditorSettingsForm } from "./editor-settings-form";

const sections = [
  { id: "editor", label: "Editor", Icon: Type, Content: EditorSettingsForm },
  { id: "agent-hooks", label: "Agent hooks", Icon: Plug, Content: AgentHooksSection },
] as const;

export function SettingsPage({ section }: { section: SettingsSection }) {
  const open = useWorkspace((state) => state.openSettings);
  const close = useWorkspace((state) => state.closeSettings);
  const heading = useRef<HTMLHeadingElement>(null);
  const content = useRef<HTMLElement>(null);
  const { label, Content } = sections.find(({ id }) => id === section) ?? sections[0];

  useLayoutEffect(() => {
    if (content.current) content.current.scrollTop = 0;
    heading.current?.focus({ preventScroll: true });
  }, [section]);

  return (
    <div className="settings-page">
      <nav className="settings-nav" aria-label="Settings navigation">
        <div className="settings-back">
          <button type="button" className="rail-row" aria-label="Back to workspace" onClick={close}>
            <ArrowLeft size={15} aria-hidden />
            Back
          </button>
        </div>
        <ul className="settings-sections">
          {sections.map(({ id, label, Icon }) => (
            <li key={id}>
              <button
                type="button"
                className="rail-row"
                aria-current={id === section ? "page" : undefined}
                onClick={() => open(id)}
              >
                <Icon size={15} aria-hidden />
                {label}
              </button>
            </li>
          ))}
        </ul>
      </nav>
      <main ref={content} className="settings-content" aria-labelledby="settings-heading">
        <div className="settings-section">
          <h2 ref={heading} id="settings-heading" tabIndex={-1}>
            {label}
          </h2>
          <Content />
        </div>
      </main>
    </div>
  );
}
