import type { EditorDocument, DocumentIdentity, Version, SyncPacket, WasmBinding } from "../document/index.mjs";

export interface JournalEntry { sequence: number; packet: SyncPacket; applied: Version }
export interface NodeStore {
  readonly nodeId: string;
  readonly durable: boolean;
  load(document: string): Promise<JournalEntry[]>;
  append(document: string, entry: JournalEntry): Promise<void>;
  close(): Promise<void>;
}
export class IndexedDbStore implements NodeStore {
  private constructor();
  static open(profile: string): Promise<IndexedDbStore>;
  readonly nodeId: string;
  readonly durable: true;
  load(document: string): Promise<JournalEntry[]>;
  append(document: string, entry: JournalEntry): Promise<void>;
  close(): Promise<void>;
}
export class MemoryStore implements NodeStore {
  readonly nodeId: string;
  readonly durable: false;
  load(document: string): Promise<JournalEntry[]>;
  append(document: string, entry: JournalEntry): Promise<void>;
  close(): Promise<void>;
}
export class EditorNode {
  constructor(options: {
    NodeBinding: new(nodeId: string, sessionId: string) => object;
    DocumentBinding: WasmBinding;
    store: NodeStore;
    nodeId?: string;
    sessionId?: string;
    onError?: (error: unknown) => void;
  });
  readonly nodeId: string;
  readonly sessionId: string;
  readonly closed: boolean;
  openDocument(options: { identity: DocumentIdentity; credential: string; seed?: SyncPacket; text?: string }): Promise<EditorDocument>;
  document(id: string): EditorDocument | undefined;
  durableVersion(id: string): Version | null;
  remoteDurableVersions(id: string): Version[];
  addLink(id: string, send: (frame: string) => void | Promise<void>, close: () => void): void;
  removeLink(id: string): void;
  receive(id: string, frame: string): void;
  /** Waits for local storage, not remote acknowledgment. */
  flush(): Promise<void>;
  /** Also disposes managed documents and releases the store/profile lock. */
  close(): Promise<void>;
}
export interface Connection { close(): void }
export function connectWebSocket(node: EditorNode, options: { url: string; token: string }): Connection;
export function connectSignaling(node: EditorNode, options: {
  url: string;
  token: string;
  iceTransportPolicy?: RTCIceTransportPolicy;
  turnTransport?: "udp" | "tcp" | "tls";
  onPeer?: (peer: { node: string; session: string; pc: RTCPeerConnection }) => void;
}): Connection;
