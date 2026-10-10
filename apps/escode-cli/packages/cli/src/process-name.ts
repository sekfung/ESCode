export const CLI_COMMAND_NAME = "escode";
export const CLI_PROCESS_NAME = "escode-cli";

interface ProcessTitleTarget {
  title: string;
}

export const setCliProcessTitle = (
  target: ProcessTitleTarget = process,
): void => {
  target.title = CLI_PROCESS_NAME;
};
