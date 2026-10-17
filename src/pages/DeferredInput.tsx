import { useEffect, useRef, useState, type KeyboardEvent } from "react";

interface Props {
  value: string;
  /// Called once, when the edit is finished (blur or Enter).
  onCommit: (v: string) => void;
  type?: "text" | "number";
  placeholder?: string;
  className?: string;
  title?: string;
  step?: string;
}

/// Text input that commits **on blur or Enter**, never on keystroke.
///
/// The router config is persisted by the backend, which validates it and
/// rewrites the preset: saving per character froze the field (and surfaced
/// half-typed values as errors — an empty alias reads as "invalid"). Escape
/// abandons the edit.
export default function DeferredInput({ value, onCommit, type = "text", ...rest }: Props) {
  const [draft, setDraft] = useState(value);
  const editing = useRef(false);

  // Follow external updates (reload, dedupe) unless the user is typing.
  useEffect(() => {
    if (!editing.current) setDraft(value);
  }, [value]);

  const commit = () => {
    editing.current = false;
    if (draft !== value) onCommit(draft);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") {
      e.currentTarget.blur();
    } else if (e.key === "Escape") {
      // Reset, then let the blur close the edit without committing.
      setDraft(value);
      editing.current = false;
      e.currentTarget.blur();
    }
  };

  return (
    <input
      {...rest}
      type={type}
      value={draft}
      onChange={(e) => {
        editing.current = true;
        setDraft(e.target.value);
      }}
      onFocus={() => {
        editing.current = true;
      }}
      onBlur={commit}
      onKeyDown={onKeyDown}
    />
  );
}
