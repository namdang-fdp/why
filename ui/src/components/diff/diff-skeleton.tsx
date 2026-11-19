export function DiffSkeleton() {
  return (
    <div className="diff-skeleton" role="status">
      <span className="sr-only">Loading diff…</span>
      {[72, 46, 65, 34, 57, 40].map((width) => (
        <span key={width} aria-hidden style={{ width: `${width}%` }} />
      ))}
    </div>
  );
}
