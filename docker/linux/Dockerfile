FROM alpine:3.22 AS certs
RUN apk add --no-cache ca-certificates

FROM scratch
COPY --from=certs /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY rflush /rflush
ENV RFLUSH_HOST=0.0.0.0
ENV RFLUSH_PORT=3000
ENV RFLUSH_DATA_DIR=/data
VOLUME ["/data"]
EXPOSE 3000
ENTRYPOINT ["/rflush"]
