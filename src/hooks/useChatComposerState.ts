import { useCallback, useState } from 'react';
import type { ChatRequestOptions } from '../features/conversations/types';
import { parseAdvancedRequestOptions } from '../features/conversations/sessionState';

export interface ComposerAttachment {
  name: string;
  type: string;
}

export function useChatComposerState() {
  const [input, setInput] = useState('');
  const [advancedRequestOptionsInput, setAdvancedRequestOptionsInput] = useState('');
  const [advancedRequestOptionsError, setAdvancedRequestOptionsError] = useState<string | null>(
    null
  );
  const [attachment, setAttachment] = useState<ComposerAttachment | null>(null);

  const resolveRequestOptions = useCallback(
    (
      showAdvancedRequestOptionsButton: boolean,
      requestOptions?: ChatRequestOptions
    ): ChatRequestOptions | null => {
      if (requestOptions) {
        return requestOptions;
      }

      if (!showAdvancedRequestOptionsButton) {
        if (advancedRequestOptionsError) {
          setAdvancedRequestOptionsError(null);
        }
        return {};
      }

      try {
        const parsedOptions = parseAdvancedRequestOptions(advancedRequestOptionsInput);
        if (advancedRequestOptionsError) {
          setAdvancedRequestOptionsError(null);
        }
        return parsedOptions;
      } catch (error) {
        const message =
          error instanceof Error ? error.message : 'composer.error.parse_failed';
        setAdvancedRequestOptionsError(message);
        return null;
      }
    },
    [advancedRequestOptionsError, advancedRequestOptionsInput]
  );

  const clearComposerDraft = useCallback(() => {
    setInput('');
    setAttachment(null);
  }, []);

  const attachPlaceholderFile = useCallback(() => {
    setAttachment({
      name: 'screenshot_data.png',
      type: 'image',
    });
  }, []);

  const removeAttachment = useCallback(() => {
    setAttachment(null);
  }, []);

  const updateAdvancedRequestOptionsInput = useCallback(
    (value: string) => {
      setAdvancedRequestOptionsInput(value);
      if (advancedRequestOptionsError) {
        setAdvancedRequestOptionsError(null);
      }
    },
    [advancedRequestOptionsError]
  );

  return {
    advancedRequestOptionsError,
    advancedRequestOptionsInput,
    attachment,
    attachPlaceholderFile,
    clearComposerDraft,
    input,
    removeAttachment,
    resolveRequestOptions,
    setInput,
    updateAdvancedRequestOptionsInput,
  };
}
