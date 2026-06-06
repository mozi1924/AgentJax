import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  Activity,
  AlertTriangle,
  BarChart3,
  CheckCircle2,
  CircuitBoard,
  Cpu,
  Database,
  FileText,
  RefreshCw,
  Shield,
  X,
} from 'lucide-react';
import { useI18n } from '../features/i18n';
import { OverlayScrollArea } from './OverlayScrollArea';
import { useActiveAgent } from '../hooks/useActiveAgent';
import { useConversationRegistry } from '../hooks/useConversationRegistry';

// ── Types matching Rust backend ────────────────────────────────────────────

interface LcmHealthResponse {
  integrity: IntegrityReport | null;
  metrics: LcmMetrics | null;
  circuitBreaker: BreakerEntry[];
  spendGuard: SpendGuardEntry[];
  config: LcmConfigData;
  repairSuggestions: string[];
}

interface IntegrityReport {
  conversationId: string;
  checks: IntegrityCheck[];
  passCount: number;
  failCount: number;
  warnCount: number;
  scannedAtUnixMs: number;
}

interface IntegrityCheck {
  name: string;
  status: 'pass' | 'fail' | 'warn';
  message: string;
  details?: unknown;
}

interface LcmMetrics {
  conversationId: string;
  messageCount: number;
  summaryCount: number;
  leafSummaryCount: number;
  condensedSummaryCount: number;
  largeFileCount: number;
  totalMessageTokens: number;
  totalSummaryTokens: number;
  collectedAtUnixMs: number;
}

interface BreakerEntry {
  key: string;
  state: 'closed' | 'open';
  consecutiveFailures: number;
  blockedUntil: number | null;
  lastFailureReason: string | null;
}

interface SpendGuardEntry {
  key: string;
  state: 'normal' | 'backingOff';
  callsInWindow: number;
  maxCalls: number;
  windowDurationSecs: number;
  backoffRemainingSecs: number | null;
}

interface LcmConfigData {
  softTokenThreshold: number;
  hardTokenThreshold: number;
  compactionTimeoutSecs: number;
  maxCompactBlockSize: number;
  truncationMaxTokens: number;
  summarizationModel: string;
  dynamicThresholds: boolean;
}

// ── Component ───────────────────────────────────────────────────────────────

interface LcmHealthModalProps {
  isOpen: boolean;
  onClose: () => void;
  conversationId?: string | null;
}

// Define the active agent hook return type
interface ActiveAgentResult {
  activeAgentId: string;
}

export default function LcmHealthModal({ isOpen, onClose, conversationId }: LcmHealthModalProps) {
  const { t } = useI18n();
  const [loading, setLoading] = useState(false);
  const [data, setData] = useState<LcmHealthResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [activeTab, setActiveTab] = useState<'integrity' | 'metrics' | 'breaker' | 'spend' | 'config'>('integrity');

  const { activeAgentId } = { activeAgentId: conversationId ? 'agent' : 'default' };

  const fetchData = useCallback(async () => {
    if (!conversationId) return;
    setLoading(true);
    setError(null);
    try {
      const result = await invoke<LcmHealthResponse>('get_lcm_health', {
        agentId: 'default',
        conversationId,
      });
      setData(result);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, [conversationId]);

  useEffect(() => {
    if (isOpen && conversationId) {
      fetchData();
    }
  }, [isOpen, conversationId, fetchData]);

  if (!isOpen) return null;

  const integrityStatusIcon = (status: string) => {
    switch (status) {
      case 'pass': return <CheckCircle2 className="h-4 w-4 text-emerald-400" />;
      case 'fail': return <AlertTriangle className="h-4 w-4 text-rose-400" />;
      case 'warn': return <AlertTriangle className="h-4 w-4 text-amber-400" />;
      default: return null;
    }
  };

  const formatTokens = (n: number) => {
    if (n >= 1000000) return `${(n / 1000000).toFixed(1)}M`;
    if (n >= 1000) return `${(n / 1000).toFixed(1)}K`;
    return String(n);
  };

  const tabs = [
    { key: 'integrity', label: 'Integrity', icon: Shield },
    { key: 'metrics', label: 'Metrics', icon: BarChart3 },
    { key: 'breaker', label: 'Circuit Breaker', icon: CircuitBoard },
    { key: 'spend', label: 'Spend Guard', icon: Cpu },
    { key: 'config', label: 'Config', icon: Activity },
  ] as const;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-sm">
      <div className="mx-auto flex h-[80vh] w-[700px] flex-col rounded-2xl border border-[#3c4043] bg-[#1a1a1c] shadow-2xl">
        {/* Header */}
        <div className="flex items-center justify-between border-b border-[#3c4043] px-5 py-3">
          <div className="flex items-center gap-2">
            <Database className="h-5 w-5 text-indigo-400" />
            <h2 className="text-base font-semibold text-slate-200">LCM Health Dashboard</h2>
          </div>
          <div className="flex items-center gap-2">
            <button
              onClick={fetchData}
              disabled={loading}
              className="flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-xs text-slate-400 transition hover:bg-[#2d2f31] hover:text-slate-200 disabled:opacity-50"
            >
              <RefreshCw className={`h-3.5 w-3.5 ${loading ? 'animate-spin' : ''}`} />
              Refresh
            </button>
            <button onClick={onClose} className="rounded-lg p-1.5 text-slate-400 transition hover:bg-[#2d2f31] hover:text-slate-200">
              <X className="h-5 w-5" />
            </button>
          </div>
        </div>

        {/* Tabs */}
        <div className="flex gap-0 border-b border-[#3c4043] px-3">
          {tabs.map(({ key, label, icon: Icon }) => (
            <button
              key={key}
              onClick={() => setActiveTab(key as typeof activeTab)}
              className={`flex items-center gap-1.5 border-b-2 px-3 py-2.5 text-xs font-medium transition ${
                activeTab === key
                  ? 'border-indigo-400 text-indigo-300'
                  : 'border-transparent text-slate-500 hover:text-slate-300'
              }`}
            >
              <Icon className="h-3.5 w-3.5" />
              {label}
            </button>
          ))}
        </div>

        {/* Content */}
        <OverlayScrollArea className="flex-1 p-4">
          {loading && !data && (
            <div className="flex items-center justify-center py-16">
              <RefreshCw className="h-8 w-8 animate-spin text-slate-500" />
            </div>
          )}

          {error && (
            <div className="rounded-xl border border-rose-500/30 bg-rose-500/10 p-4 text-sm text-rose-300">
              {error}
            </div>
          )}

          {!conversationId && (
            <div className="flex items-center justify-center py-16 text-sm text-slate-500">
              Select a conversation to view LCM health data.
            </div>
          )}

          {data && conversationId && (
            <>
              {/* Tab: Integrity */}
              {activeTab === 'integrity' && (
                <div className="space-y-3">
                  <div className="flex gap-3">
                    <div className="flex items-center gap-1.5 rounded-lg bg-emerald-500/10 px-3 py-1.5 text-xs text-emerald-400">
                      <CheckCircle2 className="h-3.5 w-3.5" />
                      {data.integrity?.passCount ?? 0} Pass
                    </div>
                    <div className="flex items-center gap-1.5 rounded-lg bg-amber-500/10 px-3 py-1.5 text-xs text-amber-400">
                      <AlertTriangle className="h-3.5 w-3.5" />
                      {data.integrity?.warnCount ?? 0} Warn
                    </div>
                    <div className="flex items-center gap-1.5 rounded-lg bg-rose-500/10 px-3 py-1.5 text-xs text-rose-400">
                      <AlertTriangle className="h-3.5 w-3.5" />
                      {data.integrity?.failCount ?? 0} Fail
                    </div>
                  </div>

                  {data.integrity?.checks.map((check) => (
                    <div
                      key={check.name}
                      className={`rounded-xl border p-3 ${
                        check.status === 'pass'
                          ? 'border-emerald-500/20 bg-emerald-500/5'
                          : check.status === 'fail'
                            ? 'border-rose-500/20 bg-rose-500/5'
                            : 'border-amber-500/20 bg-amber-500/5'
                      }`}
                    >
                      <div className="flex items-start gap-2">
                        {integrityStatusIcon(check.status)}
                        <div className="min-w-0 flex-1">
                          <div className="text-sm font-medium text-slate-200">
                            {check.name.replace(/_/g, ' ')}
                          </div>
                          <div className="mt-0.5 text-xs text-slate-400">{check.message}</div>
                        </div>
                      </div>
                    </div>
                  ))}

                  {data.repairSuggestions.length > 0 && (
                    <div className="mt-4">
                      <div className="mb-2 text-xs font-medium text-slate-400">Repair Suggestions</div>
                      {data.repairSuggestions.map((s, i) => (
                        <div key={i} className="mb-1 rounded-lg bg-indigo-500/10 px-3 py-2 text-xs text-indigo-300">
                          {s}
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              )}

              {/* Tab: Metrics */}
              {activeTab === 'metrics' && data.metrics && (
                <div className="grid grid-cols-2 gap-3">
                  <MetricCard icon={FileText} label="Messages" value={String(data.metrics.messageCount)} />
                  <MetricCard icon={Database} label="Leaf Summaries" value={String(data.metrics.leafSummaryCount)} />
                  <MetricCard icon={Database} label="Condensed Summaries" value={String(data.metrics.condensedSummaryCount)} />
                  <MetricCard icon={BarChart3} label="Large Files" value={String(data.metrics.largeFileCount)} />
                  <MetricCard icon={Activity} label="Message Tokens" value={formatTokens(data.metrics.totalMessageTokens)} />
                  <MetricCard icon={Activity} label="Summary Tokens" value={formatTokens(data.metrics.totalSummaryTokens)} />
                </div>
              )}

              {/* Tab: Circuit Breaker */}
              {activeTab === 'breaker' && (
                <div className="space-y-2">
                  {data.circuitBreaker.length === 0 && (
                    <div className="py-8 text-center text-xs text-slate-500">No circuit breaker events recorded.</div>
                  )}
                  {data.circuitBreaker.map((entry) => (
                    <div key={entry.key} className="rounded-xl border border-[#3c4043] bg-[#222226] p-3">
                      <div className="flex items-center justify-between">
                        <div className="flex items-center gap-2">
                          <div className={`h-2 w-2 rounded-full ${entry.state === 'open' ? 'bg-rose-400' : 'bg-emerald-400'}`} />
                          <span className="text-xs font-mono text-slate-300">{entry.key}</span>
                        </div>
                        <span className={`text-xs ${entry.state === 'open' ? 'text-rose-400' : 'text-emerald-400'}`}>
                          {entry.state}
                        </span>
                      </div>
                      <div className="mt-1 text-xs text-slate-500">
                        {entry.consecutiveFailures} consecutive failures
                      </div>
                    </div>
                  ))}
                </div>
              )}

              {/* Tab: Spend Guard */}
              {activeTab === 'spend' && (
                <div className="space-y-2">
                  {data.spendGuard.length === 0 && (
                    <div className="py-8 text-center text-xs text-slate-500">No spend guard events recorded.</div>
                  )}
                  {data.spendGuard.map((entry) => (
                    <div key={entry.key} className="rounded-xl border border-[#3c4043] bg-[#222226] p-3">
                      <div className="flex items-center justify-between">
                        <span className="text-xs font-mono text-slate-300">{entry.key}</span>
                        <span className={`text-xs ${entry.state === 'backingOff' ? 'text-amber-400' : 'text-emerald-400'}`}>
                          {entry.state}
                        </span>
                      </div>
                      <div className="mt-1 text-xs text-slate-500">
                        {entry.callsInWindow}/{entry.maxCalls} calls in {entry.windowDurationSecs}s window
                      </div>
                    </div>
                  ))}
                </div>
              )}

              {/* Tab: Config */}
              {activeTab === 'config' && (
                <div className="space-y-2">
                  <ConfigRow label="Soft Threshold" value={`${formatTokens(data.config.softTokenThreshold)} tokens`} />
                  <ConfigRow label="Hard Threshold" value={`${formatTokens(data.config.hardTokenThreshold)} tokens`} />
                  <ConfigRow label="Dynamic Thresholds" value={data.config.dynamicThresholds ? 'Enabled' : 'Disabled'} />
                  <ConfigRow label="Compaction Timeout" value={`${data.config.compactionTimeoutSecs}s`} />
                  <ConfigRow label="Max Block Size" value={`${data.config.maxCompactBlockSize} messages`} />
                  <ConfigRow label="Truncation Max Tokens" value={`${data.config.truncationMaxTokens} tokens`} />
                  <ConfigRow label="Summarization Model" value={data.config.summarizationModel || '(none — Level 3 only)'} />
                </div>
              )}
            </>
          )}
        </OverlayScrollArea>

        {/* Footer */}
        <div className="border-t border-[#3c4043] px-5 py-2.5 text-xs text-slate-600">
          Conversation: <span className="font-mono text-slate-500">{conversationId}</span>
        </div>
      </div>
    </div>
  );
}

// ── Sub-components ─────────────────────────────────────────────────────────

function MetricCard({ icon: Icon, label, value }: { icon: React.ComponentType<{ className?: string }>; label: string; value: string }) {
  return (
    <div className="rounded-xl border border-[#3c4043] bg-[#222226] p-4">
      <div className="flex items-center gap-2 text-xs text-slate-500">
        <Icon className="h-3.5 w-3.5" />
        {label}
      </div>
      <div className="mt-1 text-lg font-semibold text-slate-200">{value}</div>
    </div>
  );
}

function ConfigRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-center justify-between rounded-lg bg-[#222226] px-3 py-2">
      <span className="text-xs text-slate-400">{label}</span>
      <span className="text-xs font-mono text-slate-200">{value}</span>
    </div>
  );
}
