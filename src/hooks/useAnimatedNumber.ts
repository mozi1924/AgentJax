import { useEffect, useState } from 'react';

export function useAnimatedNumber(targetValue: number, durationMs = 300) {
  const [currentValue, setCurrentValue] = useState(targetValue);

  useEffect(() => {
    if (currentValue === targetValue) return;

    const startValue = currentValue;
    const distance = targetValue - startValue;
    const startTime = performance.now();

    let animationFrameId: number;

    const animate = (time: number) => {
      const elapsed = time - startTime;
      const progress = Math.min(elapsed / durationMs, 1);

      // Ease out quad
      const easeProgress = 1 - (1 - progress) * (1 - progress);
      const nextValue = Math.round(startValue + distance * easeProgress);

      setCurrentValue(nextValue);

      if (progress < 1) {
        animationFrameId = requestAnimationFrame(animate);
      } else {
        setCurrentValue(targetValue);
      }
    };

    animationFrameId = requestAnimationFrame(animate);

    return () => {
      cancelAnimationFrame(animationFrameId);
    };
  }, [targetValue, durationMs]);

  return currentValue;
}
