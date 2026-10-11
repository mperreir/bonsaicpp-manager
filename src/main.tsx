import React from "react";
import ReactDOM from "react-dom/client";
import { useTranslation } from "react-i18next";
import "./i18n";
import App from "./App";

interface BoundaryState {
  error: Error | null;
}

/// Displays the error instead of a blank screen on a render crash.
class ErrorBoundary extends React.Component<
  { children: React.ReactNode },
  BoundaryState
> {
  state: BoundaryState = { error: null };

  static getDerivedStateFromError(error: Error): BoundaryState {
    return { error };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo) {
    console.error("Render crash:", error, info.componentStack);
  }

  render() {
    if (this.state.error) {
      return <CrashScreen error={this.state.error} onRetry={() => this.setState({ error: null })} />;
    }
    return this.props.children;
  }
}

function CrashScreen({ error, onRetry }: { error: Error; onRetry: () => void }) {
  const { t } = useTranslation();
  return (
    <div style={{ padding: 24, fontFamily: "Segoe UI, sans-serif" }}>
      <h2>{t("crash.title")}</h2>
      <pre style={{ whiteSpace: "pre-wrap" }}>
        {String(error?.message ?? error)}
      </pre>
      <p>{t("crash.hint")}</p>
      <button onClick={onRetry}>{t("crash.retry")}</button>
    </div>
  );
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
);
