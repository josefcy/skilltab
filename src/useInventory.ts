import { useCallback, useEffect, useRef, useState } from "react";
import { commands, events, type Inventory } from "./bindings";

export type InventoryState = {
  inventory: Inventory | null;
  error: string | null;
  /** Wall time of the first getInventory call, as the webview saw it. */
  firstLoadMs: number | null;
  refresh: () => void;
  refreshing: boolean;
};

/** Loads the inventory once, then again whenever the backend reports a new revision (window focus, refresh). */
export function useInventory(): InventoryState {
  const [inventory, setInventory] = useState<Inventory | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [firstLoadMs, setFirstLoadMs] = useState<number | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const revision = useRef<number | null>(null);

  const load = useCallback(async (first: boolean) => {
    const started = performance.now();
    try {
      const inv = await commands.getInventory();
      if (first) setFirstLoadMs((prev) => prev ?? Math.round(performance.now() - started));
      if (inv.revision !== revision.current) {
        revision.current = inv.revision;
        setInventory(inv);
      }
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void load(true);
    const unlisten = events.inventoryChanged.listen(() => void load(false));
    return () => void unlisten.then((fn) => fn());
  }, [load]);

  const refresh = useCallback(() => {
    setRefreshing(true);
    commands
      .refresh()
      .catch((e) => setError(String(e)))
      .finally(() => setRefreshing(false));
  }, []);

  return { inventory, error, firstLoadMs, refresh, refreshing };
}
