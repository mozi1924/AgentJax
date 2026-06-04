import { useMemo, useRef, useState } from 'react';
import AppHeader from './components/AppHeader';
import Sidebar from './components/Sidebar';
import ChatArea from './components/ChatArea';
import ChatComposer from './components/ChatComposer';
import ConfirmModal from './components/ConfirmModal';
import SettingsModal from './components/SettingsModal';
import { useI18n } from './features/i18n';
import {
  canUseNativeContextMenu,
  getConversationDisplayTitle,
} from './features/conversations/conversationUtils';
import { useComposerMeasurements } from './hooks/useComposerMeasurements';
import { useContextMenuGuard } from './hooks/useContextMenuGuard';
import { useTitlebarDragging } from './hooks/useTitlebarDragging';
import { useAppConfig } from './hooks/useAppConfig';
import { useActiveAgent } from './hooks/useActiveAgent';
import { useChatSessions } from './hooks/useChatSessions';
import { useDeveloperToolsShortcut } from './hooks/useDeveloperToolsShortcut';

export default function App() {
  const { t } = useI18n();
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);

  const titlebarRef = useRef<HTMLDivElement | null>(null);
  const mainRef = useRef<HTMLDivElement | null>(null);
  const composerStageRef = useRef<HTMLDivElement | null>(null);
  const composerShellRef = useRef<HTMLDivElement | null>(null);

  // ── Agent management ──────────────────────────────────────────────────
  const {
    agents,
    activeAgentId,
    activeAgent,
    agentsLoading,
    switchAgent,
    createAgent,
    deleteAgent,
    refreshAgents,
  } = useActiveAgent();

  // ── App config (model catalog, etc.) ──────────────────────────────────
  const {
    cachePath,
    configPath,
    enableDeveloperTools,
    modelOptions,
    selectedModel,
    selectedModelOption,
    selectedReasoningMode,
    selectReasoningMode,
    setSelectedModel,
    showAdvancedRequestOptionsButton,
  } = useAppConfig({ agentId: activeAgentId });

  // ── Chat sessions (agent-scoped) ──────────────────────────────────────
  const {
    activeChatTitle,
    activeConversation,
    activeConversationId,
    activeConversationIsGenerating,
    activeConversationIsStopping,
    activeConversationIsThinking,
    advancedRequestOptionsError,
    advancedRequestOptionsInput,
    attachment,
    attachPlaceholderFile,
    cancelDeleteConversation,
    confirmDeleteConversation,
    conversationToDelete,
    createNewChat,
    generatingConversationIds,
    hasAnyGenerating,
    input,
    isEmptyConversation,
    removeAttachment,
    renameConversation,
    requestDeleteConversation,
    sendMessage,
    setActiveConversationId,
    setInput,
    sidebarConversations,
    stopActiveStream,
    updateAdvancedRequestOptionsInput,
  } = useChatSessions({
    selectedModel,
    selectedModelOption,
    selectedReasoningMode,
    showAdvancedRequestOptionsButton,
    agentId: activeAgentId,
  });

  const conversationViewKey = `${activeConversationId}-${isEmptyConversation ? 'empty' : 'messages'}`;

  const { composerHeight, emptyComposerOffset } = useComposerMeasurements({
    mainRef,
    composerStageRef,
    composerShellRef,
    attachment,
    input,
    isEmptyConversation,
  });

  useTitlebarDragging(titlebarRef);
  useContextMenuGuard(canUseNativeContextMenu);
  useDeveloperToolsShortcut(enableDeveloperTools);

  return (
    <div className="app-shell relative flex h-screen w-screen overflow-hidden bg-transparent font-sans text-slate-100 antialiased select-none">
      <div className="absolute inset-0 -z-10 overflow-hidden bg-[#131314]">
        <div
          className={`absolute left-1/2 top-1/2 h-[550px] w-[550px] -translate-x-1/2 -translate-y-1/2 animate-pulse-fast rounded-full bg-gradient-to-tr from-indigo-500/8 via-slate-600/8 to-indigo-950/12 filter blur-[80px] transition-opacity duration-1000 ease-in-out md:blur-[120px] ${
            hasAnyGenerating ? 'opacity-100' : 'pointer-events-none opacity-0'
          }`}
        />
        <div
          className={`absolute left-1/2 top-1/2 h-[500px] w-[500px] -translate-x-1/2 -translate-y-1/2 animate-pulse-slow rounded-full bg-gradient-to-tr from-slate-900/10 via-indigo-950/8 to-slate-900/10 filter blur-[80px] transition-opacity duration-1000 ease-in-out md:blur-[120px] ${
            activeConversation?.lines?.length === 0 && !activeConversationIsGenerating
              ? 'opacity-100'
              : 'pointer-events-none opacity-0'
          }`}
        />
        <div
          className={`absolute left-1/2 top-1/2 h-[300px] w-[300px] -translate-x-1/2 -translate-y-1/2 rounded-full bg-gradient-to-tr from-slate-950/10 to-slate-900/5 filter blur-[100px] transition-opacity duration-1000 ease-in-out ${
            activeConversation?.lines?.length > 0 && !activeConversationIsGenerating
              ? 'opacity-100'
              : 'pointer-events-none opacity-0'
          }`}
        />
      </div>

      <Sidebar
        isOpen={sidebarOpen}
        conversations={sidebarConversations}
        activeConversationId={activeConversationId}
        onSelectConversation={setActiveConversationId}
        onNewChat={createNewChat}
        onOpenSettings={() => setSettingsOpen(true)}
        onRenameConversation={renameConversation}
        onDeleteConversation={requestDeleteConversation}
        generatingConversationIds={generatingConversationIds}
        agents={agents}
        activeAgentId={activeAgentId}
        activeAgent={activeAgent}
        agentsLoading={agentsLoading}
        onSwitchAgent={switchAgent}
        onCreateAgent={createAgent}
        onDeleteAgent={deleteAgent}
      />

      <AppHeader
        titlebarRef={titlebarRef}
        sidebarOpen={sidebarOpen}
        onToggleSidebar={() => setSidebarOpen((prev) => !prev)}
        selectedModel={selectedModel}
        selectedModelOption={selectedModelOption}
        modelOptions={modelOptions}
        onSelectModel={setSelectedModel}
        selectedReasoningMode={selectedReasoningMode}
        onSelectReasoningMode={selectReasoningMode}
        configPath={configPath}
        cachePath={cachePath}
      />

      <main className="flex h-full flex-1 flex-col pt-12">
        <div
          ref={mainRef}
          className={`relative flex min-h-0 flex-1 flex-col transition-[margin] duration-300 ${
            sidebarOpen ? 'ml-64' : 'ml-20'
          }`}
        >
          <div
            className={`flex min-h-0 flex-1 flex-col overflow-hidden transition-opacity duration-300 ${
              isEmptyConversation ? 'pointer-events-none opacity-0' : 'opacity-100'
            }`}
            style={{ paddingBottom: isEmptyConversation ? 0 : `${composerHeight}px` }}
          >
            <div key={conversationViewKey} className="flex min-h-0 flex-1 animate-conversation-content-in">
              <ChatArea
                lines={activeConversation?.lines || []}
                isGenerating={activeConversationIsGenerating}
                isThinking={activeConversationIsThinking}
                activeChatTitle={activeChatTitle}
                contextTokenCount={activeConversation?.contextTokenCount || 0}
              />
            </div>
          </div>

          <div
            className={`pointer-events-none absolute inset-x-0 bottom-0 z-10 bg-[#131314] transition-opacity duration-700 ease-[cubic-bezier(0.22,1,0.36,1)] ${
              isEmptyConversation ? 'opacity-0' : 'opacity-100'
            }`}
            style={{ height: `${composerHeight}px` }}
          >
            <div className="pointer-events-none absolute top-0 left-0 right-0 h-10 -translate-y-full bg-gradient-to-t from-[#131314] to-transparent" />
          </div>

          <div
            ref={composerStageRef}
            className="pointer-events-none absolute inset-x-0 bottom-0 z-10 will-change-transform transition-transform duration-700 ease-[cubic-bezier(0.22,1,0.36,1)]"
            style={{
              transform: `translate3d(0, ${isEmptyConversation ? emptyComposerOffset : 0}px, 0)`,
            }}
          >
            <div className="flex w-full flex-col items-center">
              <div
                className={`mx-auto w-full max-w-3xl overflow-hidden px-4 text-center transition-[max-height,margin,opacity,transform] duration-200 ease-out md:px-8 lg:px-12 ${
                  isEmptyConversation
                    ? 'mb-6 max-h-40 translate-y-0 opacity-100 md:max-h-44'
                    : 'mb-0 max-h-0 -translate-y-2 opacity-0'
                }`}
              >
                <h1 className="text-4xl font-semibold tracking-tight md:text-5xl">
                  <span className="animate-gradient bg-gradient-to-r from-white via-slate-200 to-slate-400 bg-clip-text font-bold text-transparent">
                    {t('main.headline.mozi')}
                  </span>
                  <br />
                  <span className="text-[#444746] dark:text-[#8e9196] text-lg font-normal tracking-wide mt-2 block">
                    {t('main.headline.subtitle')}
                  </span>
                </h1>
              </div>

              <div ref={composerShellRef} className="pointer-events-auto w-full">
                <ChatComposer
                  input={input}
                  onInputChange={setInput}
                  showAdvancedRequestOptionsButton={showAdvancedRequestOptionsButton}
                  advancedRequestOptionsInput={advancedRequestOptionsInput}
                  onAdvancedRequestOptionsInputChange={updateAdvancedRequestOptionsInput}
                  advancedRequestOptionsError={advancedRequestOptionsError}
                  attachment={attachment}
                  onRemoveAttachment={removeAttachment}
                  onAttachFile={attachPlaceholderFile}
                  isGenerating={activeConversationIsGenerating}
                  isStopping={activeConversationIsStopping}
                  onSend={(text) => void sendMessage(text)}
                  onStop={stopActiveStream}
                />
              </div>
            </div>
          </div>
        </div>
      </main>

      {conversationToDelete && (
        <ConfirmModal
          title={t('confirm.delete_chat.title')}
          message={t('confirm.delete_chat.message', { title: getConversationDisplayTitle(conversationToDelete) })}
          onConfirm={confirmDeleteConversation}
          onCancel={cancelDeleteConversation}
        />
      )}

      <SettingsModal
        isOpen={settingsOpen}
        onClose={() => setSettingsOpen(false)}
        agentId={activeAgentId}
      />
    </div>
  );
}
