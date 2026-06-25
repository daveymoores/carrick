import { DynamoDBClient, PutItemCommand } from "@aws-sdk/client-dynamodb";
import axios from "axios";

const dynamo = new DynamoDBClient({ region: "eu-west-1" });

export interface AuditEvent {
  paymentId: string;
  action: string;
  timestamp: string;
}

// Writes an audit record to DynamoDB — SDK call, NOT an HTTP endpoint.
// The dynamo.send(PutItemCommand) below is an SDK-as-HTTP decoy: it uses
// the AWS SDK transport layer, not a direct HTTP fetch, and must NOT be
// extracted as a data call by the scanner.
export async function recordAuditEvent(event: AuditEvent): Promise<void> {
  await dynamo.send(
    new PutItemCommand({
      TableName: "payments-audit",
      Item: {
        paymentId: { S: event.paymentId },
        action: { S: event.action },
        timestamp: { S: event.timestamp },
      },
    })
  );

  // Real HTTP call adjacent to the SDK decoy — the scanner must extract this
  // one but must not confuse the DynamoDB SDK call above for an HTTP call.
  await axios.post(
    `${process.env.AUDIT_WEBHOOK_URL ?? "http://localhost:3099"}/audit/events`,
    event
  );
}
