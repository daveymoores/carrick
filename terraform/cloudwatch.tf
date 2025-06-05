resource "aws_sns_topic" "alerts" {
  name = "carrick-alerts"
}

resource "aws_sns_topic_subscription" "email" {
  topic_arn = aws_sns_topic.alerts.arn
  protocol  = "email"
  endpoint  = "david@carrick.tools" 
}

resource "aws_cloudwatch_metric_alarm" "lambda_invocation_alarm" {
  alarm_name          = "carrick_lambda_invocations"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "Invocations"
  namespace           = "AWS/Lambda"
  period              = 60
  statistic           = "Sum"
  threshold           = 20
  alarm_description   = "Alert if too many invocations"
  dimensions = {
    FunctionName = aws_lambda_function.check_or_upload.function_name
  }
  alarm_actions = [aws_sns_topic.alerts.arn]
}
