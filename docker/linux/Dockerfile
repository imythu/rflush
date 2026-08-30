FROM cloakhq/cloakbrowser:0.5.10

COPY rflush /rflush
ENV RFLUSH_HOST=0.0.0.0
ENV RFLUSH_PORT=3000
ENV RFLUSH_DATA_DIR=/data
VOLUME ["/data"]
EXPOSE 3000
ENTRYPOINT ["/entrypoint.sh"]
CMD ["/rflush"]
