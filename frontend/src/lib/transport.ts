// Unified transport: Tauri IPC when running inside the desktop shell,
// HTTP fetch against the local n3ur0n-server otherwise.

export interface Transport {
  invoke<T>(cmd: string, args?: unknown): Promise<T>;
}

function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

class HttpTransport implements Transport {
  async invoke<T>(cmd: string, args?: unknown): Promise<T> {
    const res = await fetch(`/api/v0/${cmd}`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: args === undefined ? undefined : JSON.stringify(args)
    });
    if (!res.ok) {
      throw new Error(`api ${cmd} failed: ${res.status} ${res.statusText}`);
    }
    return (await res.json()) as T;
  }
}

class TauriTransport implements Transport {
  async invoke<T>(cmd: string, args?: unknown): Promise<T> {
    const { invoke } = await import('@tauri-apps/api/core');
    return (await invoke(cmd, args as Record<string, unknown> | undefined)) as T;
  }
}

export const transport: Transport = isTauri() ? new TauriTransport() : new HttpTransport();
