resource "aws_s3_bucket" "carrick_types" {
  bucket = "carrick-type-cache"
  force_destroy = true
}

resource "aws_s3_bucket_public_access_block" "carrick_types" {
  bucket = aws_s3_bucket.carrick_types.id
  block_public_acls   = true
  block_public_policy = true
  ignore_public_acls  = true
  restrict_public_buckets = true
}
