// Lambda handler for async payment settlement — triggered by SQS, not HTTP.
// This export follows the AWS Lambda handler convention and must NOT be
// extracted as an HTTP endpoint by the scanner.

export interface SQSEvent {
  Records: Array<{
    body: string;
    messageId: string;
  }>;
}

export interface LambdaResult {
  batchItemFailures: Array<{ itemIdentifier: string }>;
}

// The handler signature looks like an HTTP handler but is invoked by the
// Lambda runtime from an SQS trigger — it is a Lambda handler decoy.
export const handler = async (event: SQSEvent): Promise<LambdaResult> => {
  const failures: Array<{ itemIdentifier: string }> = [];

  for (const record of event.Records) {
    try {
      const message = JSON.parse(record.body) as { paymentId: string };
      // settle logic would go here
      void message;
    } catch {
      failures.push({ itemIdentifier: record.messageId });
    }
  }

  return { batchItemFailures: failures };
};
