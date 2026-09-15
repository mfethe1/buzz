/**
 * Reducer for the persistent action-error banner (#51).
 *
 * Start failures previously surfaced only as transient toasts, so a failed
 * start (e.g. Windows installs missing buzz-acp.exe) looked like a no-op.
 * The banner keeps the last error visible until the next successful action
 * or an explicit dismiss.
 */
export type ActionBannerState = {
  /** Last non-dismissed action error, or null. */
  error: string | null;
};

export const actionBannerInitial: ActionBannerState = { error: null };

export function actionBannerReduce(
  prev: ActionBannerState,
  action:
    | {
        type: "sync";
        errorMessage: string | null;
        noticeMessage: string | null;
      }
    | { type: "dismiss" },
): ActionBannerState {
  switch (action.type) {
    case "dismiss":
      return prev.error === null ? prev : { error: null };
    case "sync": {
      if (action.errorMessage) {
        return prev.error === action.errorMessage
          ? prev
          : { error: action.errorMessage };
      }
      if (action.noticeMessage) return { error: null };
      return prev;
    }
    default:
      return prev;
  }
}
