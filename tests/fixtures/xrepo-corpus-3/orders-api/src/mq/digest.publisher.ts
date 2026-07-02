import { SQSClient, SendMessageCommand } from "@aws-sdk/client-sqs";
import { OrderDigest } from "../types/orders";

const sqs = new SQSClient({});

// Roadmap case: the queue identity lives in a URL template (env-templated
// topic). Must not be emitted as an HTTP call either.
export async function publishDailyDigest(digest: OrderDigest): Promise<void> {
  await sqs.send(
    new SendMessageCommand({
      QueueUrl: `${process.env.SQS_BASE_URL}/notifications.digest`,
      MessageBody: JSON.stringify(digest),
    })
  );
}
