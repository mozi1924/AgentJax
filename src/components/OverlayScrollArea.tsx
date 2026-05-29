import {
  forwardRef,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type HTMLAttributes,
  type Ref,
  type TextareaHTMLAttributes,
} from 'react';

type ScrollAxis = 'vertical' | 'horizontal' | 'both';

interface ScrollMetrics {
  canScrollY: boolean;
  canScrollX: boolean;
  thumbTop: number;
  thumbLeft: number;
  thumbHeight: number;
  thumbWidth: number;
}

interface DragState {
  axis: 'vertical' | 'horizontal';
  pointerStart: number;
  scrollStart: number;
  scrollMax: number;
  trackSize: number;
  thumbSize: number;
}

interface OverlayScrollbarControlsProps {
  targetRef: React.RefObject<HTMLElement | null>;
  axis: ScrollAxis;
}

interface OverlayScrollAreaProps extends HTMLAttributes<HTMLDivElement> {
  axis?: ScrollAxis;
  viewportClassName?: string;
  containerClassName?: string;
}

interface OverlayTextareaProps extends TextareaHTMLAttributes<HTMLTextAreaElement> {
  axis?: ScrollAxis;
  containerClassName?: string;
}

const MIN_THUMB_SIZE = 28;
const SCROLLBAR_EDGE_PADDING = 4;

const cx = (...parts: Array<string | false | null | undefined>) => parts.filter(Boolean).join(' ');

const assignRef = <T,>(ref: Ref<T> | undefined, value: T | null) => {
  if (!ref) return;
  if (typeof ref === 'function') {
    ref(value);
    return;
  }
  ref.current = value;
};

const axisAllowsVertical = (axis: ScrollAxis) => axis === 'vertical' || axis === 'both';
const axisAllowsHorizontal = (axis: ScrollAxis) => axis === 'horizontal' || axis === 'both';

const readMetrics = (element: HTMLElement, axis: ScrollAxis): ScrollMetrics => {
  const scrollHeight = element.scrollHeight;
  const scrollWidth = element.scrollWidth;
  const clientHeight = element.clientHeight;
  const clientWidth = element.clientWidth;
  const canScrollY = axisAllowsVertical(axis) && scrollHeight - clientHeight > 1;
  const canScrollX = axisAllowsHorizontal(axis) && scrollWidth - clientWidth > 1;

  const verticalTrackSize = Math.max(clientHeight - SCROLLBAR_EDGE_PADDING * 2, 0);
  const horizontalTrackSize = Math.max(clientWidth - SCROLLBAR_EDGE_PADDING * 2, 0);
  const thumbHeight = canScrollY
    ? Math.max(MIN_THUMB_SIZE, verticalTrackSize * (clientHeight / scrollHeight))
    : 0;
  const thumbWidth = canScrollX
    ? Math.max(MIN_THUMB_SIZE, horizontalTrackSize * (clientWidth / scrollWidth))
    : 0;

  const verticalScrollMax = Math.max(scrollHeight - clientHeight, 1);
  const horizontalScrollMax = Math.max(scrollWidth - clientWidth, 1);
  const thumbTop = canScrollY
    ? (element.scrollTop / verticalScrollMax) * Math.max(verticalTrackSize - thumbHeight, 0)
    : 0;
  const thumbLeft = canScrollX
    ? (element.scrollLeft / horizontalScrollMax) * Math.max(horizontalTrackSize - thumbWidth, 0)
    : 0;

  return {
    canScrollY,
    canScrollX,
    thumbTop,
    thumbLeft,
    thumbHeight,
    thumbWidth,
  };
};

function OverlayScrollbarControls({ targetRef, axis }: OverlayScrollbarControlsProps) {
  const [metrics, setMetrics] = useState<ScrollMetrics>({
    canScrollY: false,
    canScrollX: false,
    thumbTop: 0,
    thumbLeft: 0,
    thumbHeight: 0,
    thumbWidth: 0,
  });
  const [active, setActive] = useState(false);
  const dragStateRef = useRef<DragState | null>(null);
  const activeTimeoutRef = useRef<number | null>(null);

  const updateMetrics = useCallback(() => {
    const element = targetRef.current;
    if (!element) return;
    setMetrics(readMetrics(element, axis));
  }, [axis, targetRef]);

  const showDuringInteraction = useCallback(() => {
    setActive(true);
    if (activeTimeoutRef.current) {
      window.clearTimeout(activeTimeoutRef.current);
    }
    activeTimeoutRef.current = window.setTimeout(() => setActive(false), 900);
  }, []);

  useLayoutEffect(() => {
    updateMetrics();
  }, [updateMetrics]);

  useEffect(() => {
    const element = targetRef.current;
    if (!element) return undefined;

    let animationFrame = 0;
    const scheduleUpdate = () => {
      window.cancelAnimationFrame(animationFrame);
      animationFrame = window.requestAnimationFrame(updateMetrics);
    };
    const handleScroll = () => {
      showDuringInteraction();
      scheduleUpdate();
    };

    element.addEventListener('scroll', handleScroll, { passive: true });

    // Keep the overlay thumb in sync with layout and dynamic message/content changes.
    const resizeObserver = new ResizeObserver(scheduleUpdate);
    resizeObserver.observe(element);
    const observeChildren = () => {
      Array.from(element.children).forEach((child) => resizeObserver.observe(child));
    };
    observeChildren();

    const mutationObserver = new MutationObserver(() => {
      observeChildren();
      scheduleUpdate();
    });
    mutationObserver.observe(element, {
      childList: true,
      subtree: true,
      characterData: true,
      attributes: true,
    });

    window.addEventListener('resize', scheduleUpdate);
    scheduleUpdate();

    return () => {
      window.cancelAnimationFrame(animationFrame);
      element.removeEventListener('scroll', handleScroll);
      resizeObserver.disconnect();
      mutationObserver.disconnect();
      window.removeEventListener('resize', scheduleUpdate);
      if (activeTimeoutRef.current) {
        window.clearTimeout(activeTimeoutRef.current);
      }
    };
  }, [showDuringInteraction, targetRef, updateMetrics]);

  const beginDrag = (dragAxis: DragState['axis'], event: React.PointerEvent<HTMLDivElement>) => {
    const element = targetRef.current;
    if (!element) return;

    event.preventDefault();
    event.stopPropagation();
    const trackSize =
      dragAxis === 'vertical'
        ? Math.max(element.clientHeight - SCROLLBAR_EDGE_PADDING * 2, 0)
        : Math.max(element.clientWidth - SCROLLBAR_EDGE_PADDING * 2, 0);
    const thumbSize = dragAxis === 'vertical' ? metrics.thumbHeight : metrics.thumbWidth;

    dragStateRef.current = {
      axis: dragAxis,
      pointerStart: dragAxis === 'vertical' ? event.clientY : event.clientX,
      scrollStart: dragAxis === 'vertical' ? element.scrollTop : element.scrollLeft,
      scrollMax:
        dragAxis === 'vertical'
          ? Math.max(element.scrollHeight - element.clientHeight, 0)
          : Math.max(element.scrollWidth - element.clientWidth, 0),
      trackSize,
      thumbSize,
    };

    setActive(true);
    document.body.classList.add('overlay-scrollbar-dragging');

    const handlePointerMove = (moveEvent: PointerEvent) => {
      const dragState = dragStateRef.current;
      const target = targetRef.current;
      if (!dragState || !target) return;

      const pointer = dragState.axis === 'vertical' ? moveEvent.clientY : moveEvent.clientX;
      const trackTravel = Math.max(dragState.trackSize - dragState.thumbSize, 1);
      const nextScroll =
        dragState.scrollStart +
        ((pointer - dragState.pointerStart) / trackTravel) * dragState.scrollMax;

      if (dragState.axis === 'vertical') {
        target.scrollTop = nextScroll;
      } else {
        target.scrollLeft = nextScroll;
      }
      updateMetrics();
    };

    const endDrag = () => {
      dragStateRef.current = null;
      document.body.classList.remove('overlay-scrollbar-dragging');
      showDuringInteraction();
      window.removeEventListener('pointermove', handlePointerMove);
      window.removeEventListener('pointerup', endDrag);
      window.removeEventListener('pointercancel', endDrag);
    };

    window.addEventListener('pointermove', handlePointerMove);
    window.addEventListener('pointerup', endDrag);
    window.addEventListener('pointercancel', endDrag);
  };

  const rootStyle = useMemo(
    () => ({ '--overlay-scrollbar-opacity': active ? 1 : 0 } as CSSProperties),
    [active]
  );

  if (!metrics.canScrollY && !metrics.canScrollX) {
    return null;
  }

  return (
    <div className="overlay-scrollbar-layer" style={rootStyle} aria-hidden="true">
      {metrics.canScrollY && (
        <div className="overlay-scrollbar-track overlay-scrollbar-track-y">
          <div
            className="overlay-scrollbar-thumb overlay-scrollbar-thumb-y"
            style={{
              height: metrics.thumbHeight,
              transform: `translateY(${metrics.thumbTop}px)`,
            }}
            onPointerDown={(event) => beginDrag('vertical', event)}
          />
        </div>
      )}
      {metrics.canScrollX && (
        <div className="overlay-scrollbar-track overlay-scrollbar-track-x">
          <div
            className="overlay-scrollbar-thumb overlay-scrollbar-thumb-x"
            style={{
              width: metrics.thumbWidth,
              transform: `translateX(${metrics.thumbLeft}px)`,
            }}
            onPointerDown={(event) => beginDrag('horizontal', event)}
          />
        </div>
      )}
    </div>
  );
}

export const OverlayScrollArea = forwardRef<HTMLDivElement, OverlayScrollAreaProps>(
  (
    {
      axis = 'vertical',
      children,
      className,
      containerClassName,
      viewportClassName,
      ...viewportProps
    },
    forwardedRef
  ) => {
    const viewportRef = useRef<HTMLDivElement | null>(null);
    const setViewportRef = useCallback(
      (node: HTMLDivElement | null) => {
        viewportRef.current = node;
        assignRef(forwardedRef, node);
      },
      [forwardedRef]
    );

    return (
      <div className={cx('overlay-scrollbar-root', containerClassName)}>
        <div
          {...viewportProps}
          ref={setViewportRef}
          data-overlay-scroll-axis={axis}
          className={cx('overlay-scrollbar-scrollport', className, viewportClassName)}
        >
          {children}
        </div>
        <OverlayScrollbarControls targetRef={viewportRef} axis={axis} />
      </div>
    );
  }
);

OverlayScrollArea.displayName = 'OverlayScrollArea';

export const OverlayTextarea = forwardRef<HTMLTextAreaElement, OverlayTextareaProps>(
  ({ axis = 'vertical', className, containerClassName, value, onChange, ...textareaProps }, forwardedRef) => {
    const textareaRef = useRef<HTMLTextAreaElement | null>(null);
    const setTextareaRef = useCallback(
      (node: HTMLTextAreaElement | null) => {
        textareaRef.current = node;
        assignRef(forwardedRef, node);
      },
      [forwardedRef]
    );

    return (
      <div className={cx('overlay-scrollbar-root', containerClassName)}>
        <textarea
          {...textareaProps}
          ref={setTextareaRef}
          value={value}
          onChange={onChange}
          data-overlay-scroll-axis={axis}
          className={cx('overlay-scrollbar-scrollport', className)}
        />
        <OverlayScrollbarControls targetRef={textareaRef} axis={axis} />
      </div>
    );
  }
);

OverlayTextarea.displayName = 'OverlayTextarea';
