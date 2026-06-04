import { Component, type ErrorInfo, type ReactNode } from 'react';
import { AlertTriangle } from 'lucide-react';

interface SettingsErrorBoundaryProps {
  children: ReactNode;
  /** Optional identifier shown in the error message to help locate the source. */
  contextLabel?: string;
  /** When true, renders a compact inline error instead of a full block. */
  inline?: boolean;
  /** Custom fallback rendered in place of children on error. */
  fallback?: ReactNode;
}

interface SettingsErrorBoundaryState {
  hasError: boolean;
  error: Error | null;
}

export class SettingsErrorBoundary extends Component<
  SettingsErrorBoundaryProps,
  SettingsErrorBoundaryState
> {
  constructor(props: SettingsErrorBoundaryProps) {
    super(props);
    this.state = { hasError: false, error: null };
  }

  static getDerivedStateFromError(error: Error): SettingsErrorBoundaryState {
    return { hasError: true, error };
  }

  componentDidCatch(error: Error, errorInfo: ErrorInfo): void {
    console.error(
      `[SettingsErrorBoundary${this.props.contextLabel ? ` (${this.props.contextLabel})` : ''}]`,
      error,
      errorInfo
    );
  }

  render() {
    if (!this.state.hasError) {
      return this.props.children;
    }

    if (this.props.fallback !== undefined) {
      return this.props.fallback;
    }

    const label = this.props.contextLabel;
    const message = this.state.error?.message || 'Unknown error';

    return (
      <div className="rounded-lg border border-rose-500/20 bg-rose-950/10 px-3 py-2.5">
        <div className="flex items-start gap-2">
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-rose-400" />
          <div className="min-w-0">
            <p className="text-[12px] font-medium text-rose-300">
              {label
                ? `Error in “${label}”`
                : 'A settings section encountered an error'}
            </p>
            <p className="mt-0.5 truncate text-[11px] text-rose-400/70">{message}</p>
          </div>
        </div>
      </div>
    );
  }
}
