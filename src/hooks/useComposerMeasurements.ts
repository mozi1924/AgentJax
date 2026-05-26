import { useLayoutEffect, useState } from 'react';

export function useComposerMeasurements({
  mainRef,
  composerStageRef,
  composerShellRef,
  attachment,
  input,
  isEmptyConversation,
}: {
  mainRef: React.RefObject<HTMLDivElement | null>;
  composerStageRef: React.RefObject<HTMLDivElement | null>;
  composerShellRef: React.RefObject<HTMLDivElement | null>;
  attachment: unknown;
  input: string;
  isEmptyConversation: boolean;
}) {
  const [composerHeight, setComposerHeight] = useState(0);
  const [emptyComposerOffset, setEmptyComposerOffset] = useState(0);

  useLayoutEffect(() => {
    const mainElement = mainRef.current;
    const composerStageElement = composerStageRef.current;
    const composerShellElement = composerShellRef.current;
    if (!mainElement || !composerStageElement || !composerShellElement) {
      return undefined;
    }

    const updateMeasurements = () => {
      const mainBounds = mainElement.getBoundingClientRect();
      const composerBounds = composerShellElement.getBoundingClientRect();
      const stageBounds = composerStageElement.getBoundingClientRect();

      const nextComposerHeight = composerBounds.height;
      const centeredTop = Math.max(0, (mainBounds.height - stageBounds.height) / 2);
      const dockedTop = Math.max(0, mainBounds.height - stageBounds.height);
      const nextEmptyOffset = centeredTop - dockedTop;

      setComposerHeight((previousHeight) =>
        Math.abs(previousHeight - nextComposerHeight) > 0.5 ? nextComposerHeight : previousHeight
      );
      setEmptyComposerOffset((previousOffset) =>
        Math.abs(previousOffset - nextEmptyOffset) > 0.5 ? nextEmptyOffset : previousOffset
      );
    };

    updateMeasurements();

    const resizeObserver = new ResizeObserver(() => {
      updateMeasurements();
    });

    resizeObserver.observe(mainElement);
    resizeObserver.observe(composerStageElement);
    resizeObserver.observe(composerShellElement);

    return () => {
      resizeObserver.disconnect();
    };
  }, [attachment, input, isEmptyConversation]);

  return { composerHeight, emptyComposerOffset };
}

