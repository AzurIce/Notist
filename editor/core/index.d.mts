export type JsonValue = null | boolean | number | string | readonly JsonValue[] | { readonly [key: string]: JsonValue };
export interface DocumentIdentity { readonly document_id: string; readonly history_id: string }
export interface Version { readonly identity: DocumentIdentity; readonly clocks: Readonly<Record<string, number>> }
export interface TextSnapshot { readonly text: string; readonly version: Version; readonly revision: number }
/** UTF-16, half-open ranges in BEFORE text; sorted and disjoint. */
export interface TextEdit { readonly from: number; readonly to: number; readonly insert: string }
export interface UndoState { readonly can_undo: boolean; readonly can_redo: boolean; readonly group_open: boolean }
export type ChangeCause = { readonly kind: "local" | "import"; readonly origin: string } | { readonly kind: "undo" | "redo" | "history_cleared" };
export interface ChangeEvent {
  readonly cause: ChangeCause;
  readonly before: TextSnapshot;
  readonly after: TextSnapshot;
  readonly edits: readonly TextEdit[];
  readonly undo: UndoState;
  readonly restored_metadata: JsonValue;
  readonly restored_positions: readonly number[];
}
export interface SyncPacket { identity: DocumentIdentity; kind: "snapshot" | "updates"; data: number[] }
/** Opaque JSON-serializable token. Obtain from anchorAt; retain and pass through. */
export interface Anchor { readonly __anchor: unique symbol }
export interface ResolvedAnchor { readonly offset: number; readonly refreshed: Anchor }
export interface WasmBinding {
  new(identity: string, writer: string | undefined, initial: string): object;
  from_snapshot(packet: string, writer?: string): object;
}
export interface HostOptions { writer?: string; onListenerError?: (error: unknown) => void }
export class CoreError extends Error { readonly code: string; readonly offset?: number }
export class EditorCore {
  private constructor();
  static create(Binding: WasmBinding, options: HostOptions & { identity: DocumentIdentity; text?: string }): EditorCore;
  static restore(Binding: WasmBinding, packet: SyncPacket, options?: HostOptions): EditorCore;
  snapshot(): TextSnapshot;
  readonly writerId: string;
  readonly undoState: UndoState;
  transact(transaction: { expectedVersion: Version; edits: readonly TextEdit[]; origin?: string; undoMetadata?: JsonValue; undoPositions?: readonly number[] }): ChangeEvent | null;
  undo(metadata?: JsonValue, positions?: readonly number[]): ChangeEvent | null;
  redo(metadata?: JsonValue, positions?: readonly number[]): ChangeEvent | null;
  beginUndoGroup(): void;
  endUndoGroup(): void;
  clearUndo(): void;
  exportSnapshot(): SyncPacket;
  exportUpdatesSince(version: Version): SyncPacket;
  import(packet: SyncPacket, origin?: string): { readonly event: ChangeEvent | null; readonly pending: boolean };
  anchorAt(offset: number, affinity?: "before" | "after"): Anchor;
  resolveAnchor(anchor: Anchor): ResolvedAnchor;
  subscribe(callback: (event: ChangeEvent) => void): () => void;
  dispose(): void;
}
