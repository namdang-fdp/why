import { Search, X } from "lucide-react";
import { useWorkspace } from "@/stores/workspace-context";

/** The filter box at the top of the rail. `/` focuses it; Esc clears it, then leaves it. */
export function RailFilter() {
  const filter = useWorkspace((state) => state.railFilter);
  const setFilter = useWorkspace((state) => state.setRailFilter);
  return (
    <div className="rail-filter" role="search">
      <Search size={13} aria-hidden />
      <input
        id="rail-filter"
        type="text"
        value={filter}
        placeholder="Filter"
        aria-label="Filter repositories, worktrees and files"
        aria-keyshortcuts="/"
        spellCheck={false}
        autoComplete="off"
        onChange={(event) => setFilter(event.target.value)}
        onKeyDown={(event) => {
          if (event.key !== "Escape") return;
          event.preventDefault();
          event.stopPropagation();
          if (filter) setFilter("");
          else event.currentTarget.blur();
        }}
      />
      {filter ? (
        <button
          type="button"
          className="rail-filter-clear"
          aria-label="Clear filter"
          onClick={() => {
            setFilter("");
            document.getElementById("rail-filter")?.focus();
          }}
        >
          <X size={12} aria-hidden />
        </button>
      ) : (
        <kbd aria-hidden>/</kbd>
      )}
    </div>
  );
}
