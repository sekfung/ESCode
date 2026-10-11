import {
  DEFAULT_FEEDBACK_TICKET_FRAMEWORK,
  type ESCodeProvider,
  type FeedbackTicketFramework,
} from "@escode/shared";

export function inferFeedbackFrameworkFromAgentProvider(
  _provider: ESCodeProvider | null | undefined,
): FeedbackTicketFramework {
  return DEFAULT_FEEDBACK_TICKET_FRAMEWORK;
}
