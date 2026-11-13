import { useEffect, useState } from "react";
import { commands, type AliasSuggestionView, type JstackView } from "./bindings";

/** Reloads whenever the inventory revision moves, so a pull, a fetch or a confirmed alias shows at once. undefined means loading. */
function useByRevision<T>(revision: number, load: () => Promise<T>): T | undefined {
  const [data, setData] = useState<T | undefined>(undefined);
  useEffect(() => {
    let live = true;
    load()
      .then((d) => live && setData(d))
      .catch(() => live && setData(undefined));
    return () => {
      live = false;
    };
    // `load` is a fresh closure each render, and the revision is the only thing that changes what it returns.
  }, [revision]);
  return data;
}

/** null when this machine has no jstack clone. */
export function useJstack(revision: number): JstackView | null | undefined {
  return useByRevision(revision, () => commands.getJstack());
}

export function useAliasSuggestions(revision: number): AliasSuggestionView[] | undefined {
  return useByRevision(revision, () => commands.aliasSuggestions());
}
