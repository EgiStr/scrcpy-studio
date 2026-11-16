// Keeps one page's render error from blanking the whole window: the error is shown in
// place of the page, and the menu and device dock keep working.
import { Component, type ReactNode } from "react";

export class ErrorBoundary extends Component<{ children: ReactNode }, { error?: Error }> {
  state: { error?: Error } = {};

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="h-full overflow-y-auto p-6 text-sm">
        <p className="mb-2 text-red-400">This page stopped with an error.</p>
        <pre className="mb-3 whitespace-pre-wrap rounded border border-zinc-800 bg-black p-3 font-mono text-xs text-zinc-300">
          {error.message}
          {"\n"}
          {error.stack}
        </pre>
        <button className="rounded border border-zinc-700 px-3 py-1 text-zinc-200" onClick={() => this.setState({ error: undefined })}>
          Try again
        </button>
      </div>
    );
  }
}
