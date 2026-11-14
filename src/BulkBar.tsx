type Props = {
  selected: number;
  /** Selected rows the current filters hide. They are still deleted, so the bar says so. */
  hidden: number;
  onDelete: () => void;
  onClear: () => void;
};

export function BulkBar({ selected, hidden, onDelete, onClear }: Props) {
  return (
    <div className="bulkbar" role="region" aria-label="Selected skills">
      <span className="bulk-count" aria-live="polite">
        {selected} selected
        {hidden > 0 && <span className="sub">, {hidden} hidden by filters</span>}
      </span>
      <button type="button" className="manage danger" onClick={onDelete}>
        Delete…
      </button>
      <button type="button" className="link" onClick={onClear}>
        Clear
      </button>
    </div>
  );
}
